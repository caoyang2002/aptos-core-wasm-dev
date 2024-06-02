// Copyright © Aptos Foundation
// Parts of the project are originally copyright © Meta Platforms, Inc.
// SPDX-License-Identifier: Apache-2.0

use crate::{
    block_storage::{
        block_tree::BlockTree,
        pending_blocks::PendingBlocks,
        tracing::{observe_block, BlockStage},
        BlockReader,
    },
    counters,
    payload_manager::PayloadManager,
    persistent_liveness_storage::{
        PersistentLivenessStorage, RecoveryData, RootInfo, RootMetadata,
    },
    pipeline::execution_client::TExecutionClient,
    util::time_service::TimeService,
};
use anyhow::{bail, ensure, format_err, Context};
use aptos_consensus_types::{
    block::Block, common::Round, pipelined_block::PipelinedBlock, quorum_cert::QuorumCert,
    sync_info::SyncInfo, timeout_2chain::TwoChainTimeoutCertificate,
    wrapped_ledger_info::WrappedLedgerInfo,
};
use aptos_crypto::{hash::ACCUMULATOR_PLACEHOLDER_HASH, HashValue};
use aptos_executor_types::StateComputeResult;
use aptos_infallible::{Mutex, RwLock};
use aptos_logger::prelude::*;
use aptos_types::ledger_info::LedgerInfoWithSignatures;
use futures::executor::block_on;
#[cfg(any(test, feature = "fuzzing"))]
use std::sync::atomic::{AtomicBool, Ordering};
use std::{sync::Arc, time::Duration};

#[cfg(test)]
#[path = "block_store_test.rs"]
mod block_store_test;

#[path = "sync_manager.rs"]
pub mod sync_manager;

fn update_counters_for_ordered_blocks(ordered_blocks: &[Arc<PipelinedBlock>]) {
    for block in ordered_blocks {
        observe_block(block.block().timestamp_usecs(), BlockStage::ORDERED);
    }
}

/// Responsible for maintaining all the blocks of payload and the dependencies of those blocks
/// (parent and previous QC links).  It is expected to be accessed concurrently by multiple threads
/// and is thread-safe.
///
/// Example tree block structure based on parent links.
///                         ╭--> A3
/// Genesis--> B0--> B1--> B2--> B3
///             ╰--> C1--> C2
///                         ╰--> D3
///
/// Example corresponding tree block structure for the QC links (must follow QC constraints).
///                         ╭--> A3
/// Genesis--> B0--> B1--> B2--> B3
///             ├--> C1
///             ├--------> C2
///             ╰--------------> D3
pub struct BlockStore {
    inner: Arc<RwLock<BlockTree>>,
    execution_client: Arc<dyn TExecutionClient>,
    /// The persistent storage backing up the in-memory data structure, every write should go
    /// through this before in-memory tree.
    storage: Arc<dyn PersistentLivenessStorage>,
    /// Used to ensure that any block stored will have a timestamp < the local time
    time_service: Arc<dyn TimeService>,
    // consistent with round type
    vote_back_pressure_limit: Round,
    payload_manager: Arc<PayloadManager>,
    #[cfg(any(test, feature = "fuzzing"))]
    back_pressure_for_test: AtomicBool,
    order_vote_enabled: bool,
    pending_blocks: Arc<Mutex<PendingBlocks>>,
}

impl BlockStore {
    pub fn new(
        storage: Arc<dyn PersistentLivenessStorage>,
        initial_data: RecoveryData,
        execution_client: Arc<dyn TExecutionClient>,
        max_pruned_blocks_in_mem: usize,
        time_service: Arc<dyn TimeService>,
        vote_back_pressure_limit: Round,
        payload_manager: Arc<PayloadManager>,
        order_vote_enabled: bool,
        pending_blocks: Arc<Mutex<PendingBlocks>>,
    ) -> Self {
        let highest_2chain_tc = initial_data.highest_2chain_timeout_certificate();
        let (root, root_metadata, blocks, quorum_certs) = initial_data.take();
        let block_store = block_on(Self::build(
            root,
            root_metadata,
            blocks,
            quorum_certs,
            highest_2chain_tc,
            execution_client,
            storage,
            max_pruned_blocks_in_mem,
            time_service,
            vote_back_pressure_limit,
            payload_manager,
            order_vote_enabled,
            pending_blocks,
        ));
        block_on(block_store.try_send_for_execution());
        block_store
    }

    async fn try_send_for_execution(&self) {
        // reproduce the same batches (important for the commit phase)
        let mut certs = self.inner.read().get_all_quorum_certs_with_commit_info();
        certs.sort_unstable_by_key(|qc| qc.commit_info().round());
        for qc in certs {
            if qc.commit_info().round() > self.commit_root().round() {
                info!(
                    "trying to commit to round {} with ledger info {}",
                    qc.commit_info().round(),
                    qc.ledger_info()
                );

                if let Err(e) = self.send_for_execution(qc.into_wrapped_ledger_info()).await {
                    error!("Error in try-committing blocks. {}", e.to_string());
                }
            }
        }
    }

    async fn build(
        root: RootInfo,
        root_metadata: RootMetadata,
        blocks: Vec<Block>,
        quorum_certs: Vec<QuorumCert>,
        highest_2chain_timeout_cert: Option<TwoChainTimeoutCertificate>,
        execution_client: Arc<dyn TExecutionClient>,
        storage: Arc<dyn PersistentLivenessStorage>,
        max_pruned_blocks_in_mem: usize,
        time_service: Arc<dyn TimeService>,
        vote_back_pressure_limit: Round,
        payload_manager: Arc<PayloadManager>,
        order_vote_enabled: bool,
        pending_blocks: Arc<Mutex<PendingBlocks>>,
    ) -> Self {
        let RootInfo(root_block, root_qc, root_ordered_cert, root_commit_cert) = root;

        //verify root is correct
        assert!(
            // decoupled execution allows dummy versions
            root_qc.certified_block().version() == 0
                || root_qc.certified_block().version() == root_metadata.version(),
            "root qc version {} doesn't match committed trees {}",
            root_qc.certified_block().version(),
            root_metadata.version(),
        );
        assert!(
            // decoupled execution allows dummy executed_state_id
            root_qc.certified_block().executed_state_id() == *ACCUMULATOR_PLACEHOLDER_HASH
                || root_qc.certified_block().executed_state_id() == root_metadata.accu_hash,
            "root qc state id {} doesn't match committed trees {}",
            root_qc.certified_block().executed_state_id(),
            root_metadata.accu_hash,
        );

        let result = StateComputeResult::new(
            root_metadata.accu_hash,
            root_metadata.frozen_root_hashes,
            root_metadata.num_leaves, /* num_leaves */
            vec![],                   /* parent_root_hashes */
            0,                        /* parent_num_leaves */
            None,                     /* epoch_state */
            vec![],                   /* compute_status */
            vec![],                   /* txn_infos */
            vec![],                   /* reconfig_events */
        );

        let pipelined_root_block = PipelinedBlock::new(
            *root_block,
            vec![],
            // Create a dummy state_compute_result with necessary fields filled in.
            result,
        );

        let tree = BlockTree::new(
            pipelined_root_block,
            root_qc,
            root_ordered_cert,
            root_commit_cert,
            max_pruned_blocks_in_mem,
            highest_2chain_timeout_cert.map(Arc::new),
        );

        let block_store = Self {
            inner: Arc::new(RwLock::new(tree)),
            execution_client,
            storage,
            time_service,
            vote_back_pressure_limit,
            payload_manager,
            #[cfg(any(test, feature = "fuzzing"))]
            back_pressure_for_test: AtomicBool::new(false),
            order_vote_enabled,
            pending_blocks,
        };

        for block in blocks {
            block_store.insert_block(block).await.unwrap_or_else(|e| {
                panic!("[BlockStore] failed to insert block during build {:?}", e)
            });
        }
        for qc in quorum_certs {
            block_store
                .insert_single_quorum_cert(qc)
                .unwrap_or_else(|e| {
                    panic!("[BlockStore] failed to insert quorum during build{:?}", e)
                });
        }

        counters::LAST_COMMITTED_ROUND.set(block_store.ordered_root().round() as i64);
        block_store
    }

    /// Send an ordered block id with the proof for execution, returns () on success or error
    pub async fn send_for_execution(
        &self,
        finality_proof: WrappedLedgerInfo,
    ) -> anyhow::Result<()> {
        let block_id_to_commit = finality_proof.commit_info().id();
        let block_to_commit = self
            .get_block(block_id_to_commit)
            .ok_or_else(|| format_err!("Committed block id not found"))?;

        // First make sure that this commit is new.
        ensure!(
            block_to_commit.round() > self.ordered_root().round(),
            "Committed block round lower than root"
        );

        let blocks_to_commit = self
            .path_from_ordered_root(block_id_to_commit)
            .unwrap_or_default();

        assert!(!blocks_to_commit.is_empty());

        let block_tree = self.inner.clone();
        let storage = self.storage.clone();
        let finality_proof_clone = finality_proof.clone();
        self.pending_blocks
            .lock()
            .gc(finality_proof.commit_info().round());
        // This callback is invoked synchronously with and could be used for multiple batches of blocks.
        self.execution_client
            .finalize_order(
                &blocks_to_commit,
                finality_proof.ledger_info().clone(),
                Box::new(
                    move |committed_blocks: &[Arc<PipelinedBlock>],
                          commit_decision: LedgerInfoWithSignatures| {
                        block_tree.write().commit_callback(
                            storage,
                            committed_blocks,
                            finality_proof,
                            commit_decision,
                        );
                    },
                ),
            )
            .await
            .expect("Failed to persist commit");

        self.inner
            .write()
            .insert_ordered_cert(finality_proof_clone.clone());
        update_counters_for_ordered_blocks(&blocks_to_commit);

        Ok(())
    }

    pub async fn rebuild(
        &self,
        root: RootInfo,
        root_metadata: RootMetadata,
        blocks: Vec<Block>,
        quorum_certs: Vec<QuorumCert>,
        order_vote_enabled: bool,
    ) {
        info!(
            "Rebuilding block tree. root {:?}, blocks {:?}, qcs {:?}",
            root,
            blocks.iter().map(|b| b.id()).collect::<Vec<_>>(),
            quorum_certs
                .iter()
                .map(|qc| qc.certified_block().id())
                .collect::<Vec<_>>()
        );
        let max_pruned_blocks_in_mem = self.inner.read().max_pruned_blocks_in_mem();
        // Rollover the previous highest TC from the old tree to the new one.
        let prev_2chain_htc = self
            .highest_2chain_timeout_cert()
            .map(|tc| tc.as_ref().clone());
        let BlockStore { inner, .. } = Self::build(
            root,
            root_metadata,
            blocks,
            quorum_certs,
            prev_2chain_htc,
            self.execution_client.clone(),
            Arc::clone(&self.storage),
            max_pruned_blocks_in_mem,
            Arc::clone(&self.time_service),
            self.vote_back_pressure_limit,
            self.payload_manager.clone(),
            order_vote_enabled,
            self.pending_blocks.clone(),
        )
        .await;

        // Unwrap the new tree and replace the existing tree.
        *self.inner.write() = Arc::try_unwrap(inner)
            .unwrap_or_else(|_| panic!("New block tree is not shared"))
            .into_inner();
        self.try_send_for_execution().await;
    }

    /// Insert a block if it passes all validation tests.
    /// Returns the Arc to the block kept in the block store after persisting it to storage
    ///
    /// This function assumes that the ancestors are present (returns MissingParent otherwise).
    ///
    /// Duplicate inserts will return the previously inserted block (
    /// note that it is considered a valid non-error case, for example, it can happen if a validator
    /// receives a certificate for a block that is currently being added).
    pub async fn insert_block(&self, block: Block) -> anyhow::Result<Arc<PipelinedBlock>> {
        if let Some(existing_block) = self.get_block(block.id()) {
            return Ok(existing_block);
        }
        ensure!(
            self.inner.read().ordered_root().round() < block.round(),
            "Block with old round"
        );

        let pipelined_block = PipelinedBlock::new_ordered(block.clone());
        // ensure local time past the block time
        let block_time = Duration::from_micros(pipelined_block.timestamp_usecs());
        let current_timestamp = self.time_service.get_current_timestamp();
        if let Some(t) = block_time.checked_sub(current_timestamp) {
            if t > Duration::from_secs(1) {
                warn!(
                    "Long wait time {}ms for block {}",
                    t.as_millis(),
                    pipelined_block.block()
                );
            }
            self.time_service.wait_until(block_time).await;
        }
        if let Some(payload) = pipelined_block.block().payload() {
            self.payload_manager
                .prefetch_payload_data(payload, pipelined_block.block().timestamp_usecs());
        }
        self.storage
            .save_tree(vec![pipelined_block.block().clone()], vec![])
            .context("Insert block failed when saving block")?;
        self.inner.write().insert_block(pipelined_block)
    }

    /// Validates quorum certificates and inserts it into block tree assuming dependencies exist.
    pub fn insert_single_quorum_cert(&self, qc: QuorumCert) -> anyhow::Result<()> {
        // If the parent block is not the root block (i.e not None), ensure the executed state
        // of a block is consistent with its QuorumCert, otherwise persist the QuorumCert's
        // state and on restart, a new execution will agree with it.  A new execution will match
        // the QuorumCert's state on the next restart will work if there is a memory
        // corruption, for example.
        match self.get_block(qc.certified_block().id()) {
            Some(pipelined_block) => {
                ensure!(
                    // decoupled execution allows dummy block infos
                    pipelined_block
                        .block_info()
                        .match_ordered_only(qc.certified_block()),
                    "QC for block {} has different {:?} than local {:?}",
                    qc.certified_block().id(),
                    qc.certified_block(),
                    pipelined_block.block_info()
                );
                observe_block(
                    pipelined_block.block().timestamp_usecs(),
                    BlockStage::QC_ADDED,
                );
            },
            None => bail!("Insert {} without having the block in store first", qc),
        };

        self.storage
            .save_tree(vec![], vec![qc.clone()])
            .context("Insert block failed when saving quorum")?;
        self.inner.write().insert_quorum_cert(qc)
    }

    /// Replace the highest 2chain timeout certificate in case the given one has a higher round.
    /// In case a timeout certificate is updated, persist it to storage.
    pub fn insert_2chain_timeout_certificate(
        &self,
        tc: Arc<TwoChainTimeoutCertificate>,
    ) -> anyhow::Result<()> {
        let cur_tc_round = self
            .highest_2chain_timeout_cert()
            .map_or(0, |tc| tc.round());
        if tc.round() <= cur_tc_round {
            return Ok(());
        }
        self.storage
            .save_highest_2chain_timeout_cert(tc.as_ref())
            .context("Timeout certificate insert failed when persisting to DB")?;
        self.inner.write().replace_2chain_timeout_cert(tc);
        Ok(())
    }

    #[cfg(test)]
    fn commit(&self, committed_blocks: &[Arc<PipelinedBlock>], finality_proof: WrappedLedgerInfo) {
        let commit_proof = finality_proof.ledger_info().clone();
        self.inner.write().commit_callback(
            self.storage.clone(),
            committed_blocks,
            finality_proof,
            commit_proof,
        );
    }

    #[cfg(any(test, feature = "fuzzing"))]
    pub fn set_back_pressure_for_test(&self, back_pressure: bool) {
        self.back_pressure_for_test
            .store(back_pressure, Ordering::Relaxed)
    }

    /// Return if the consensus is backpressured
    fn vote_back_pressure(&self) -> bool {
        #[cfg(any(test, feature = "fuzzing"))]
        {
            if self.back_pressure_for_test.load(Ordering::Relaxed) {
                return true;
            }
        }
        let commit_round = self.commit_root().round();
        let ordered_round = self.ordered_root().round();
        counters::OP_COUNTERS
            .gauge("back_pressure")
            .set((ordered_round - commit_round) as i64);
        ordered_round > self.vote_back_pressure_limit + commit_round
    }

    pub fn pending_blocks(&self) -> Arc<Mutex<PendingBlocks>> {
        self.pending_blocks.clone()
    }

    pub fn pipeline_pending_latency(&self, proposal_timestamp: Duration) -> Duration {
        let ordered_root = self.ordered_root();
        let commit_root = self.commit_root();
        let pending_path = self
            .path_from_commit_root(self.ordered_root().id())
            .unwrap_or_default();
        let pending_rounds = pending_path.len();
        let oldest_not_committed = pending_path.into_iter().min_by_key(|b| b.round());

        let oldest_not_committed_spent_in_pipeline = oldest_not_committed
            .as_ref()
            .and_then(|b| b.elapsed_in_pipeline())
            .unwrap_or(Duration::ZERO);

        let ordered_round = ordered_root.round();
        let oldest_not_committed_round = oldest_not_committed.as_ref().map_or(0, |b| b.round());
        let commit_round = commit_root.round();
        let ordered_timestamp = Duration::from_micros(ordered_root.timestamp_usecs());
        let oldest_not_committed_timestamp = oldest_not_committed
            .as_ref()
            .map(|b| Duration::from_micros(b.timestamp_usecs()))
            .unwrap_or(Duration::ZERO);
        let committed_timestamp = Duration::from_micros(commit_root.timestamp_usecs());
        let commit_cert_timestamp =
            Duration::from_micros(self.highest_commit_cert().commit_info().timestamp_usecs());

        fn latency_from_proposal(proposal_timestamp: Duration, timestamp: Duration) -> Duration {
            if timestamp.is_zero() {
                // latency not known without non-genesis blocks
                Duration::ZERO
            } else {
                proposal_timestamp.checked_sub(timestamp).unwrap()
            }
        }

        let latency_to_committed = latency_from_proposal(proposal_timestamp, committed_timestamp);
        let latency_to_oldest_not_committed =
            latency_from_proposal(proposal_timestamp, oldest_not_committed_timestamp);
        let latency_to_ordered = latency_from_proposal(proposal_timestamp, ordered_timestamp);

        info!(
            pending_rounds = pending_rounds,
            ordered_round = ordered_round,
            oldest_not_committed_round = oldest_not_committed_round,
            commit_round = commit_round,
            oldest_not_committed_spent_in_pipeline =
                oldest_not_committed_spent_in_pipeline.as_millis() as u64,
            latency_to_ordered_ms = latency_to_ordered.as_millis() as u64,
            latency_to_oldest_not_committed = latency_to_oldest_not_committed.as_millis() as u64,
            latency_to_committed_ms = latency_to_committed.as_millis() as u64,
            latency_to_commit_cert_ms =
                latency_from_proposal(proposal_timestamp, commit_cert_timestamp).as_millis() as u64,
            "Pipeline pending latency on proposal creation",
        );

        counters::CONSENSUS_PROPOSAL_PENDING_ROUNDS.observe(pending_rounds as f64);
        counters::CONSENSUS_PROPOSAL_PENDING_DURATION
            .observe(oldest_not_committed_spent_in_pipeline.as_secs_f64());

        if pending_rounds > 1 {
            // TODO cleanup
            // previous logic was using difference between committed and ordered.
            // keeping it until we test out the new logic.
            // latency_to_oldest_not_committed
            //     .saturating_sub(latency_to_ordered.min(MAX_ORDERING_PIPELINE_LATENCY_REDUCTION))

            oldest_not_committed_spent_in_pipeline
        } else {
            Duration::ZERO
        }
    }
}

impl BlockReader for BlockStore {
    fn block_exists(&self, block_id: HashValue) -> bool {
        self.inner.read().block_exists(&block_id)
    }

    fn get_block(&self, block_id: HashValue) -> Option<Arc<PipelinedBlock>> {
        self.inner.read().get_block(&block_id)
    }

    fn ordered_root(&self) -> Arc<PipelinedBlock> {
        self.inner.read().ordered_root()
    }

    fn commit_root(&self) -> Arc<PipelinedBlock> {
        self.inner.read().commit_root()
    }

    fn get_quorum_cert_for_block(&self, block_id: HashValue) -> Option<Arc<QuorumCert>> {
        self.inner.read().get_quorum_cert_for_block(&block_id)
    }

    fn path_from_ordered_root(&self, block_id: HashValue) -> Option<Vec<Arc<PipelinedBlock>>> {
        self.inner.read().path_from_ordered_root(block_id)
    }

    fn path_from_commit_root(&self, block_id: HashValue) -> Option<Vec<Arc<PipelinedBlock>>> {
        self.inner.read().path_from_commit_root(block_id)
    }

    fn highest_certified_block(&self) -> Arc<PipelinedBlock> {
        self.inner.read().highest_certified_block()
    }

    fn highest_quorum_cert(&self) -> Arc<QuorumCert> {
        self.inner.read().highest_quorum_cert()
    }

    fn highest_ordered_cert(&self) -> Arc<WrappedLedgerInfo> {
        self.inner.read().highest_ordered_cert()
    }

    fn highest_commit_cert(&self) -> Arc<WrappedLedgerInfo> {
        self.inner.read().highest_commit_cert()
    }

    fn highest_2chain_timeout_cert(&self) -> Option<Arc<TwoChainTimeoutCertificate>> {
        self.inner.read().highest_2chain_timeout_cert()
    }

    fn sync_info(&self) -> SyncInfo {
        SyncInfo::new_decoupled(
            self.highest_quorum_cert().as_ref().clone(),
            self.highest_ordered_cert().as_ref().clone(),
            self.highest_commit_cert().as_ref().clone(),
            self.highest_2chain_timeout_cert()
                .map(|tc| tc.as_ref().clone()),
        )
    }

    fn vote_back_pressure(&self) -> bool {
        self.vote_back_pressure()
    }

    fn pipeline_pending_latency(&self, proposal_timestamp: Duration) -> Duration {
        self.pipeline_pending_latency(proposal_timestamp)
    }
}

#[cfg(any(test, feature = "fuzzing"))]
impl BlockStore {
    /// Returns the number of blocks in the tree
    pub(crate) fn len(&self) -> usize {
        self.inner.read().len()
    }

    /// Returns the number of child links in the tree
    pub(crate) fn child_links(&self) -> usize {
        self.inner.read().child_links()
    }

    /// The number of pruned blocks that are still available in memory
    pub(super) fn pruned_blocks_in_mem(&self) -> usize {
        self.inner.read().pruned_blocks_in_mem()
    }

    /// Helper function to insert the block with the qc together
    pub async fn insert_block_with_qc(&self, block: Block) -> anyhow::Result<Arc<PipelinedBlock>> {
        self.insert_single_quorum_cert(block.quorum_cert().clone())?;
        if self.ordered_root().round() < block.quorum_cert().commit_info().round() {
            self.send_for_execution(block.quorum_cert().into_wrapped_ledger_info())
                .await?;
        }
        self.insert_block(block).await
    }
}

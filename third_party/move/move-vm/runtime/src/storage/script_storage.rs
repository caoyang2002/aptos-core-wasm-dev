// Copyright (c) The Move Contributors
// SPDX-License-Identifier: Apache-2.0

use crate::loader::Script;
use move_binary_format::{errors::PartialVMResult, file_format::CompiledScript};
use sha3::{Digest, Sha3_256};
use std::sync::Arc;

pub fn script_hash(serialized_script: &[u8]) -> [u8; 32] {
    let mut sha3_256 = Sha3_256::new();
    sha3_256.update(serialized_script);
    sha3_256.finalize().into()
}

/// Represents storage which caches scripts, executed so far. The clients can
/// implement this trait to ensure that even script dependency is upgraded, the
/// correct script is still returned. Scripts are cached based on their hash.
pub trait ScriptStorage {
    /// Returns a deserialized script, either by directly deserializing it from the
    /// provided bytes, or fetching it from the storage (if it has been cached). Note
    /// that there are no guarantees that the returned script is verified. An error
    /// is returned if the deserialization fails.
    fn fetch_deserialized_script(
        &self,
        serialized_script: &[u8],
    ) -> PartialVMResult<Arc<CompiledScript>>;

    /// Returns a verified script, if it is cached. If not, the script is created using
    /// the passed callback function. It is the responsibility of the client to ensure
    /// that the callback verifies the script. An error is returned if script fails to
    /// deserialize or verify.
    fn fetch_or_create_verified_script(
        &self,
        serialized_script: &[u8],
        f: &dyn Fn(Arc<CompiledScript>) -> PartialVMResult<Script>,
    ) -> PartialVMResult<Arc<Script>>;
}
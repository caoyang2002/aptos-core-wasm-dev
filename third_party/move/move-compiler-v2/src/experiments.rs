// Copyright © Aptos Foundation
// Parts of the project are originally copyright © Meta Platforms, Inc.
// SPDX-License-Identifier: Apache-2.0

//! Defines 'experiments' (flags) for the compiler. Most phases of the
//! compiler can be enabled or disabled via an experiment. An experiment
//! can be set via the command line (`--experiment name[=on/off]`),
//! via an environment variable (`MVC_EXP=def,..` where `def` is
//! `name[=on/off]`), or programmatically. Experiments are retrieved
//! via `options.experiment_on(NAME)`.
//!
//! The declaration of experiments happens via the datatype `Experiment`
//! which defines its name, description, and default value. The default
//! can be either be a fixed constant or inherited from another
//! experiment, effectively allowing to activate a group of experiments
//! via some meta-experiment. For example, the `OPTIMIZE` experiment
//! turns on or off a bunch of other experiments, unless those are
//! defined explicitly.

use once_cell::sync::Lazy;
use std::collections::BTreeMap;

#[derive(Clone)]
pub struct Experiment {
    /// The name of the experiment
    pub name: String,
    /// A description of the experiment
    pub description: String,
    /// Whether the default is true or false
    pub default: DefaultValue,
}

#[derive(Clone)]
pub enum DefaultValue {
    /// Whether the default is a fixed value.
    Given(bool),
    /// Whether the default is inherited from another experiment
    Inherited(String),
}

pub static EXPERIMENTS: Lazy<BTreeMap<String, Experiment>> = Lazy::new(|| {
    use DefaultValue::*;
    let experiments = vec![
        Experiment {
            name: Experiment::CHECKS.to_string(),
            description: "Turns on or off a group of context checks".to_string(),
            default: Given(true),
        },
        Experiment {
            name: Experiment::REFERENCE_SAFETY.to_string(),
            description: "Turns on or off reference safety check error reporting".to_string(),
            default: Inherited(Experiment::CHECKS.to_string()),
        },
        Experiment {
            name: Experiment::USAGE_CHECK.to_string(),
            description: "Turns on or off checks for correct usage of types and variables"
                .to_string(),
            default: Inherited(Experiment::CHECKS.to_string()),
        },
        Experiment {
            name: Experiment::UNINITIALIZED_CHECK.to_string(),
            description: "Turns on or off checks for uninitialized variables".to_string(),
            default: Inherited(Experiment::CHECKS.to_string()),
        },
        Experiment {
            name: Experiment::KEEP_UNINIT_ANNOTATIONS.to_string(),
            description: "Determines whether the annotations for \
            uninitialized variable analysis should be kept around (for testing)"
                .to_string(),
            default: Given(false),
        },
        Experiment {
            name: Experiment::ABILITY_CHECK.to_string(),
            description: "Turns on or off ability checks".to_string(),
            default: Inherited(Experiment::CHECKS.to_string()),
        },
        Experiment {
            name: Experiment::ACCESS_CHECK.to_string(),
            description: "Turns on or off access and use checks".to_string(),
            default: Inherited(Experiment::CHECKS.to_string()),
        },
        Experiment {
            name: Experiment::ACQUIRES_CHECK.to_string(),
            description: "Turns on or off v1 style acquires checks".to_string(),
            default: Inherited(Experiment::CHECKS.to_string()),
        },
        Experiment {
            name: Experiment::SEQS_IN_BINOPS_CHECK.to_string(),
            description: "Turns on or off checks for sequences within binary operations"
                .to_string(),
            default: Inherited(Experiment::CHECKS.to_string()),
        },
        Experiment {
            name: Experiment::INLINING.to_string(),
            description: "Turns on or off inlining".to_string(),
            default: Given(true),
        },
        Experiment {
            name: Experiment::SPEC_CHECK.to_string(),
            description: "Turns on or off specification checks".to_string(),
            default: Inherited(Experiment::CHECKS.to_string()),
        },
        Experiment {
            name: Experiment::SPEC_REWRITE.to_string(),
            description: "Turns on or off specification rewriting".to_string(),
            default: Given(false),
        },
        Experiment {
            name: Experiment::LAMBDA_LIFTING.to_string(),
            description: "Turns on or off lambda lifting".to_string(),
            default: Given(false),
        },
        Experiment {
            name: Experiment::RECURSIVE_TYPE_CHECK.to_string(),
            description: "Turns on or off checking of recursive structs and type instantiations"
                .to_string(),
            default: Inherited(Experiment::CHECKS.to_string()),
        },
        Experiment {
            name: Experiment::SPLIT_CRITICAL_EDGES.to_string(),
            description: "Turns on or off splitting of critical edges".to_string(),
            default: Given(false),
        },
        Experiment {
            name: Experiment::OPTIMIZE.to_string(),
            description: "Turns on or off a group of optimizations".to_string(),
            default: Given(true),
        },
        Experiment {
            name: Experiment::COPY_PROPAGATION.to_string(),
            description: "Whether copy propagation is run".to_string(),
            default: Given(false),
        },
        Experiment {
            name: Experiment::DEAD_CODE_ELIMINATION.to_string(),
            description: "Whether to run dead store and unreachable code elimination".to_string(),
            default: Inherited(Experiment::OPTIMIZE.to_string()),
        },
        Experiment {
            name: Experiment::PEEPHOLE_OPTIMIZATION.to_string(),
            description: "Whether to run peephole optimization on generated file format"
                .to_string(),
            default: Inherited(Experiment::OPTIMIZE.to_string()),
        },
        Experiment {
            name: Experiment::UNUSED_STRUCT_PARAMS_CHECK.to_string(),
            description: "Whether to check for unused struct type parameters".to_string(),
            default: Inherited(Experiment::CHECKS.to_string()),
        },
        Experiment {
            name: Experiment::UNUSED_ASSIGNMENT_CHECK.to_string(),
            description: "Whether to check for unused assignments".to_string(),
            default: Inherited(Experiment::CHECKS.to_string()),
        },
        Experiment {
            name: Experiment::VARIABLE_COALESCING.to_string(),
            description: "Whether to run variable coalescing".to_string(),
            default: Inherited(Experiment::OPTIMIZE.to_string()),
        },
        Experiment {
            name: Experiment::VARIABLE_COALESCING_ANNOTATE.to_string(),
            description: "Whether to run variable coalescing, annotation only (for testing)"
                .to_string(),
            default: Given(false),
        },
        Experiment {
            name: Experiment::KEEP_INLINE_FUNS.to_string(),
            description: "Whether to keep functions after inlining \
            or remove them from the model"
                .to_string(),
            default: Given(true),
        },
        Experiment {
            name: Experiment::AST_SIMPLIFY.to_string(),
            description: "Whether to run the ast simplifier".to_string(),
            default: Inherited(Experiment::OPTIMIZE.to_string()),
        },
        Experiment {
            name: Experiment::AST_SIMPLIFY_FULL.to_string(),
            description: "Whether to run the ast simplifier, including code elimination"
                .to_string(),
            default: Given(false),
        },
        Experiment {
            name: Experiment::GEN_ACCESS_SPECIFIERS.to_string(),
            description: "Whether to generate access specifiers in the file format.\
             This is currently off by default to mitigate bug #12623."
                .to_string(),
            default: Given(false),
        },
        Experiment {
            name: Experiment::ATTACH_COMPILED_MODULE.to_string(),
            description: "Whether to attach the compiled module to the global env.".to_string(),
            default: Given(false),
        },
        Experiment {
            name: Experiment::INSTRUCTION_REORDERING.to_string(),
            description: "Whether to run instruction reordering transformation".to_string(),
            default: Inherited(Experiment::OPTIMIZE.to_string()),
        },
    ];
    experiments
        .into_iter()
        .map(|e| (e.name.clone(), e))
        .collect()
});

/// For documentation of the constants here, see the definition of `EXPERIMENTS`.
impl Experiment {
    pub const ABILITY_CHECK: &'static str = "ability-check";
    pub const ACCESS_CHECK: &'static str = "access-use-function-check";
    pub const ACQUIRES_CHECK: &'static str = "acquires-check";
    pub const AST_SIMPLIFY: &'static str = "ast-simplify";
    pub const AST_SIMPLIFY_FULL: &'static str = "ast-simplify-full";
    pub const ATTACH_COMPILED_MODULE: &'static str = "attach-compiled-module";
    pub const CHECKS: &'static str = "checks";
    pub const COPY_PROPAGATION: &'static str = "copy-propagation";
    pub const DEAD_CODE_ELIMINATION: &'static str = "dead-code-elimination";
    pub const DUPLICATE_STRUCT_PARAMS_CHECK: &'static str = "duplicate-struct-params-check";
    pub const GEN_ACCESS_SPECIFIERS: &'static str = "gen-access-specifiers";
    pub const INLINING: &'static str = "inlining";
    pub const KEEP_INLINE_FUNS: &'static str = "keep-inline-funs";
    pub const KEEP_UNINIT_ANNOTATIONS: &'static str = "keep-uninit-annotations";
    pub const LAMBDA_LIFTING: &'static str = "lambda-lifting";
    pub const OPTIMIZE: &'static str = "optimize";
    pub const PEEPHOLE_OPTIMIZATION: &'static str = "peephole-optimization";
    pub const RECURSIVE_TYPE_CHECK: &'static str = "recursive-type-check";
    pub const REFERENCE_SAFETY: &'static str = "reference-safety";
    pub const SEQS_IN_BINOPS_CHECK: &'static str = "seqs-in-binops-check";
    pub const SPEC_CHECK: &'static str = "spec-check";
    pub const SPEC_REWRITE: &'static str = "spec-rewrite";
    pub const SPLIT_CRITICAL_EDGES: &'static str = "split-critical-edges";
    pub const UNINITIALIZED_CHECK: &'static str = "uninitialized-check";
    pub const UNUSED_ASSIGNMENT_CHECK: &'static str = "unused-assignment-check";
    pub const UNUSED_STRUCT_PARAMS_CHECK: &'static str = "unused-struct-params-check";
    pub const USAGE_CHECK: &'static str = "usage-check";
    pub const VARIABLE_COALESCING: &'static str = "variable-coalescing";
    pub const VARIABLE_COALESCING_ANNOTATE: &'static str = "variable-coalescing-annotate";
    pub const INSTRUCTION_REORDERING: &'static str = "instruction-reordering";
}

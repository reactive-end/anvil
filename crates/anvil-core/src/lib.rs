//! Core domain contracts, configuration, classification, resilience, and log compression for `anvil`.

pub mod check_cache;
pub mod circuit_breaker;
pub mod compressor;
pub mod config;
pub mod diff_classifier;
pub mod diff_grep;
pub mod fs_util;
pub mod git;
pub mod ratchet;
pub mod stack_detector;
pub mod stats;
pub mod traits;

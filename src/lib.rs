//! Analyzer for `-ftime-trace` output.
//!
//! This crate reads the trace files that clang writes for each translation
//! unit. It reconstructs the compile-time event tree and finds build-time
//! bottlenecks. The main entry points are [`analyzer::analyze_build_dir`] and
//! [`report::write_report`].

pub mod analyzer;
pub mod build_info;
pub mod build_traces;
pub mod cli;
pub mod header_usage;
pub mod interner;
pub mod raw_trace;
pub mod report;

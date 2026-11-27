//! Shared helpers for the end-to-end fixture tests.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use build_time_insight::analyzer::{AnalysisResult, TemplateInstantiation, UnitAnalysis};
use build_time_insight::build_info::BuildInfo;
use build_time_insight::build_traces;
use build_time_insight::interner::StringId;

/// Returns the sorted build directories under a fixture that contain traces.
pub fn fixture_build_dirs(fixture: &str) -> Vec<PathBuf> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(fixture);
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(root)
        .map(|rd| {
            rd.filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .filter(|path| {
                    let is_build_dir = path.is_dir()
                        && path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .is_some_and(|name| name.starts_with("build-"));
                    is_build_dir && contains_traces(path)
                })
                .collect()
        })
        .unwrap_or_default();
    dirs.sort();
    dirs
}

fn contains_traces(dir: &Path) -> bool {
    !build_traces::glob_trace_paths(dir.to_str().unwrap_or(""), &[]).is_empty()
}

pub fn resolve(strings: &[String], id: StringId) -> &str {
    id.resolve(strings)
}

/// Find a header id whose path ends with `suffix`.
pub fn header_id(strings: &[String], result: &AnalysisResult, suffix: &str) -> Option<StringId> {
    result
        .header_analysis
        .keys()
        .find(|id| resolve(strings, **id).ends_with(suffix))
        .copied()
}

pub fn unit_names(strings: &[String], result: &AnalysisResult) -> Vec<String> {
    result
        .unit_analysis
        .keys()
        .map(|id| resolve(strings, *id).to_string())
        .collect()
}

/// Checks that a build directory's recorded compiler matches its name, e.g.
/// `build-clang-18` must report clang 18.x.
pub fn assert_build_info_matches_dir(dir: &Path, info: &BuildInfo) {
    let expected_major = dir
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix("build-clang-"))
        .unwrap_or_else(|| panic!("unexpected fixture build dir name: {}", dir.display()));

    let toolchain = info
        .toolchain
        .as_ref()
        .unwrap_or_else(|| panic!("no toolchain info in {}", dir.display()));
    let compiler = toolchain
        .cxx_compiler
        .as_ref()
        .unwrap_or_else(|| panic!("no C++ compiler info in {}", dir.display()));

    assert_eq!(compiler.name, "Clang", "{}", dir.display());
    assert!(
        compiler.version == expected_major
            || compiler.version.starts_with(&format!("{expected_major}.")),
        "{} reports clang {}, expected major {expected_major}",
        dir.display(),
        compiler.version
    );
}

/// Every target graph must exist and targets with sources need compile time.
pub fn assert_target_durations(dir: &Path, result: &AnalysisResult) {
    let graph = result
        .targets
        .as_ref()
        .unwrap_or_else(|| panic!("no CMake target data in {}", dir.display()));
    for target in &graph.targets {
        if target.source_count > 0 {
            assert!(
                target.duration > 0,
                "target '{}' has sources but no duration in {}",
                target.name,
                dir.display()
            );
        }
    }
}

/// Aggregation invariants: every aggregated value must equal the sum of the
/// per-unit values it was derived from, and all durations must be sane.
pub fn assert_aggregation_invariants(strings: &[String], result: &AnalysisResult) {
    for (hid, header) in &result.header_analysis {
        let mut sum = 0i64;
        let mut count = 0i64;
        for unit in result.unit_analysis.values() {
            let chains = unit.header_inclusion_paths.get(hid);
            let has_usage = unit.header_usage.contains_key(hid);
            if !unit.header_parse_duration.contains_key(hid) && chains.is_none() && !has_usage {
                continue;
            }
            sum += unit.header_parse_duration.get(hid).copied().unwrap_or(0);
            // One entry per `Source` span, not one per unit: a header can be
            // entered several times in the same translation unit. Headers that
            // have only usage data (no trace span) count once for the unit.
            count += chains
                .map(|chains| chains.values().copied().sum::<i64>())
                .filter(|&n| n > 0)
                .unwrap_or(1);
        }
        assert_eq!(
            header.parse_duration,
            sum,
            "header '{}' parse_duration != sum of per-unit parse times",
            resolve(strings, *hid)
        );
        assert_eq!(
            header.num_inclusions,
            count,
            "header '{}' num_inclusions != sum of per-unit inclusion events",
            resolve(strings, *hid)
        );
        let usage_total = header.unused_units
            + header.forward_declarable_units
            + header.definition_required_units;
        assert!(
            usage_total <= header.num_inclusions as usize,
            "header '{}' usage counts exceed inclusions",
            resolve(strings, *hid)
        );
    }

    for (uid, unit) in &result.unit_analysis {
        let name = resolve(strings, *uid);
        assert!(unit.total_duration >= 0, "unit '{name}' negative total");
        assert!(
            unit.frontend_duration >= 0,
            "unit '{name}' negative frontend"
        );
        assert!(unit.backend_duration >= 0, "unit '{name}' negative backend");
        assert!(
            unit.total_duration >= unit.frontend_duration + unit.backend_duration,
            "unit '{name}' total < frontend + backend"
        );
    }

    assert_template_totals(
        strings,
        result,
        &result.function_template_instantiations,
        &|unit: &UnitAnalysis| &unit.function_template_instantiations,
    );
    assert_template_totals(
        strings,
        result,
        &result.class_template_instantiations,
        &|unit: &UnitAnalysis| &unit.class_template_instantiations,
    );
}

fn assert_template_totals(
    strings: &[String],
    result: &AnalysisResult,
    build_map: &HashMap<StringId, TemplateInstantiation>,
    per_unit: &dyn Fn(&UnitAnalysis) -> &HashMap<StringId, TemplateInstantiation>,
) {
    for (tid, template) in build_map {
        let (mut duration, mut count) = (0i64, 0i64);
        for unit in result.unit_analysis.values() {
            if let Some(unit_template) = per_unit(unit).get(tid) {
                duration += unit_template.total_duration;
                count += unit_template.num_instantiations;
            }
        }
        assert_eq!(
            template.total_duration,
            duration,
            "template '{}' build total_duration != sum of per-unit",
            resolve(strings, *tid)
        );
        assert_eq!(
            template.num_instantiations,
            count,
            "template '{}' build count != sum of per-unit",
            resolve(strings, *tid)
        );
    }
}

//! End-to-end validation against the C++20 modules fixture project.
//!
//! Header usage is intentionally not tested here. The committed module build
//! directories are sanitized, which removes compiler-specific `.pcm` BMIs and
//! CMake `.modmap` response files. Replaying `--header-usage` for module units
//! would therefore require rebuilding the modules first.

use std::path::{Path, PathBuf};

use build_time_insight::analyzer::{self, AnalysisResult, UnitAnalysis};
use build_time_insight::build_info::BuildInfo;
use build_time_insight::report;
use build_time_insight::report::Report;

mod support;

use support::{
    assert_aggregation_invariants, assert_build_info_matches_dir, assert_target_durations,
    header_id, resolve, unit_names,
};

const FIXTURE: &str = "tests/data/cpp-modules-project";

fn fixture_build_dirs() -> Vec<PathBuf> {
    support::fixture_build_dirs(FIXTURE)
}

fn unit_by_suffix<'a>(
    strings: &[String],
    result: &'a AnalysisResult,
    suffix: &str,
) -> Option<&'a UnitAnalysis> {
    result
        .unit_analysis
        .iter()
        .find(|(id, _)| resolve(strings, **id).ends_with(suffix))
        .map(|(_, unit)| unit)
}

fn assert_module_structure(strings: &[String], result: &AnalysisResult) {
    let units = unit_names(strings, result);
    assert_eq!(units.len(), 6, "unexpected module units: {units:?}");
    for unit in &units {
        assert!(
            !unit.ends_with(".pcm"),
            "PCM phase leaked into units: {unit}"
        );
        assert!(
            !unit.ends_with(".json"),
            "trace file leaked into units: {unit}"
        );
    }

    for expected in [
        "src/core.cppm",
        "src/core-shapes.cppm",
        "src/core-detail.cppm",
        "src/core.cpp",
        "src/main.cpp",
        "src/consumer.cpp",
    ] {
        assert!(
            units.iter().any(|unit| unit.ends_with(expected)),
            "missing expected module unit {expected}; got: {units:?}"
        );
    }

    // Interface units must include their PCM-producing frontend phase, not only
    // the cheap PCM-to-object backend trace emitted by clang <= 21.
    for expected in ["src/core.cppm", "src/core-shapes.cppm"] {
        let unit = unit_by_suffix(strings, result, expected)
            .unwrap_or_else(|| panic!("missing module interface unit {expected}"));
        assert!(
            unit.total_duration >= 10_000,
            "{expected} duration is suspiciously small: {} us",
            unit.total_duration
        );
        assert!(
            unit.frontend_duration > unit.backend_duration,
            "{expected} is not frontend-dominated: frontend={} backend={}",
            unit.frontend_duration,
            unit.backend_duration
        );
    }

    for suffix in ["include/util.h", "include/legacy.h"] {
        let hid = header_id(strings, result, suffix)
            .unwrap_or_else(|| panic!("expected header ending in '{suffix}'"));
        let header = &result.header_analysis[&hid];
        assert!(header.parse_duration > 0, "'{suffix}' has no parse time");
        assert!(header.num_inclusions >= 1, "'{suffix}' never included");
    }

    let has_std_header = result.header_analysis.keys().any(|id| {
        let path = resolve(strings, *id);
        path.ends_with("/string") || path.ends_with("/vector")
    });
    assert!(
        has_std_header,
        "expected STL headers from the global module fragment"
    );

    // Clang does not report the module-local Vec<double> instantiation in every
    // version, so only require that template-instantiation tracking is present.
    assert!(
        result
            .class_template_instantiations
            .values()
            .any(|instantiation| instantiation.total_duration > 0),
        "expected class template instantiations in the module build"
    );
}

fn assert_module_targets(dir: &Path, result: &AnalysisResult) {
    assert_target_durations(dir, result);

    let graph = result
        .targets
        .as_ref()
        .unwrap_or_else(|| panic!("no CMake target data in {}", dir.display()));
    let names: Vec<&str> = graph
        .targets
        .iter()
        .map(|target| target.name.as_str())
        .collect();
    assert!(names.contains(&"core"), "targets: {names:?}");
    assert!(names.contains(&"modules_app"), "targets: {names:?}");
    assert!(
        graph
            .edges
            .iter()
            .any(|edge| edge.from == "modules_app" && edge.to == "core"),
        "expected a modules_app -> core edge, got: {:?}",
        graph
            .edges
            .iter()
            .map(|edge| format!("{}->{}", edge.from, edge.to))
            .collect::<Vec<_>>()
    );
}

#[test]
fn modules_build_dirs_report_correct_data() {
    let dirs = fixture_build_dirs();
    if dirs.is_empty() {
        eprintln!(
            "SKIP: no module build dirs found; run tests/data/cpp-modules-project/build-traces.sh --nix"
        );
        return;
    }

    for dir in &dirs {
        eprintln!("analyzing module build dir: {}", dir.display());
        let analysis =
            analyzer::analyze_build_dir(dir.to_str().unwrap(), 4, &[], false, false).unwrap();
        assert_aggregation_invariants(&analysis.strings, &analysis.result);
        assert_module_structure(&analysis.strings, &analysis.result);
        assert_module_targets(dir, &analysis.result);

        let info = build_time_insight::build_info::extract_build_info(
            dir.to_str().expect("fixture path is valid UTF-8"),
        );
        assert_build_info_matches_dir(dir, &info);
    }
}

#[test]
fn modules_report_round_trips() {
    let dirs = fixture_build_dirs();
    if dirs.is_empty() {
        eprintln!(
            "SKIP: no module build dirs found; run tests/data/cpp-modules-project/build-traces.sh --nix"
        );
        return;
    }

    for dir in &dirs {
        let analysis =
            analyzer::analyze_build_dir(dir.to_str().unwrap(), 4, &[], false, false).unwrap();
        let report = Report::new(
            analysis.strings.clone(),
            BuildInfo::default(),
            analysis.result,
        );
        let path = std::env::temp_dir().join(format!(
            "bti_modules_roundtrip_{}.bin",
            dir.file_name().unwrap().to_string_lossy()
        ));
        report::write_report(&report, &path).unwrap();
        let read = report::read_report(&path).unwrap();
        std::fs::remove_file(&path).ok();

        assert_aggregation_invariants(&read.strings, &read.analysis);
        assert_module_structure(&read.strings, &read.analysis);
        assert_module_targets(dir, &read.analysis);
    }
}

#[test]
fn modules_target_graph_is_correct() {
    let dirs = fixture_build_dirs();
    if dirs.is_empty() {
        eprintln!(
            "SKIP: no module build dirs found; run tests/data/cpp-modules-project/build-traces.sh --nix"
        );
        return;
    }

    for dir in &dirs {
        let analysis =
            analyzer::analyze_build_dir(dir.to_str().unwrap(), 4, &[], false, false).unwrap();
        assert_module_targets(dir, &analysis.result);
    }
}

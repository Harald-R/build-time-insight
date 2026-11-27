//! End-to-end validation against the C++ fixture project in `tests/data/cpp-general-project`.
//!
//! The fixture is built with clang's `-ftime-trace` (see
//! `tests/data/cpp-general-project/build-traces.sh`), producing one build directory per
//! compiler. These tests analyze each such directory and assert that the reported
//! data is correct.

use std::path::{Path, PathBuf};

use rayon::prelude::*;

use build_time_insight::analyzer::{self, AnalysisResult};
use build_time_insight::build_info::BuildInfo;
use build_time_insight::header_usage::HeaderUsage;
use build_time_insight::report;
use build_time_insight::report::Report;

mod support;

use support::{
    assert_aggregation_invariants, assert_build_info_matches_dir, assert_target_durations,
    header_id, resolve, unit_names,
};

const FIXTURE: &str = "tests/data/cpp-general-project";

fn fixture_build_dirs() -> Vec<PathBuf> {
    support::fixture_build_dirs(FIXTURE)
}

// ---------------------------------------------------------------------------
// Correctness checks
// ---------------------------------------------------------------------------

/// The fixture's expected shape: the core units and headers are present, the
/// headers took measurable parse time, and the known inclusion chain exists.
fn assert_fixture_structure(strings: &[String], result: &AnalysisResult) {
    let units = unit_names(strings, result);
    for expected in ["main.cpp", "geometry.cpp", "shapes.cpp"] {
        assert!(
            units.iter().any(|n| n.ends_with(&format!("/{expected}"))),
            "missing expected unit {expected}; got: {units:?}"
        );
    }

    for suffix in [
        "include/geometry.h",
        "include/vector3.h",
        "include/shapes.h",
    ] {
        let hid = header_id(strings, result, suffix)
            .unwrap_or_else(|| panic!("expected header ending in '{suffix}'"));
        let h = &result.header_analysis[&hid];
        assert!(h.parse_duration > 0, "'{suffix}' has no parse time");
        assert!(h.num_inclusions >= 1, "'{suffix}' never included");
    }

    // The fixture includes vector3.h through geometry.h in every unit.
    let geometry = header_id(strings, result, "include/geometry.h").expect("geometry.h");
    let vector3 = header_id(strings, result, "include/vector3.h").expect("vector3.h");
    let has_chain = result.header_analysis[&vector3]
        .inclusion_paths
        .keys()
        .any(|chain| chain.contains(&geometry));
    assert!(
        has_chain,
        "expected a geometry.h -> vector3.h inclusion chain"
    );

    // main.cpp does the most work (driver TU), and is frontend-dominated.
    let main = result
        .unit_analysis
        .iter()
        .find(|(id, _)| resolve(strings, **id).ends_with("/main.cpp"))
        .expect("main.cpp unit")
        .1;
    assert!(main.frontend_duration > main.backend_duration);
}

/// The edge-case units must be classified exactly as expected. Direct includes
/// are discovered from the compiler, so headers that clang's time trace omitted
/// are still classified.
fn assert_header_usage(strings: &[String], result: &AnalysisResult) {
    let has_any_usage = result
        .unit_analysis
        .values()
        .any(|u| !u.header_usage.is_empty());
    assert!(
        has_any_usage,
        "header-usage analysis produced no data; the compile command could not be re-run"
    );

    // Only direct includes can be removed from a unit. A transitive header must
    // never be classified here.
    for (uid, u) in &result.unit_analysis {
        let name = resolve(strings, *uid);
        for hid in u.header_usage.keys() {
            let direct = u
                .header_inclusion_paths
                .get(hid)
                .is_none_or(|paths| paths.keys().any(|chain| chain.is_empty()));
            assert!(
                direct,
                "unit '{name}' classifies transitive header '{}'",
                resolve(strings, *hid)
            );
        }
    }

    let expected: &[(&str, &str, HeaderUsage)] = &[
        // Unused direct includes.
        ("/edge_unused.cpp", "include/unused.h", HeaderUsage::Unused),
        ("/shapes.cpp", "include/utils.h", HeaderUsage::Unused),
        // Used only through declarations.
        (
            "/edge_fwd.cpp",
            "include/fwd_class.h",
            HeaderUsage::DeclarationOnly,
        ),
        (
            "/edge_unused.cpp",
            "include/geometry.h",
            HeaderUsage::DeclarationOnly,
        ),
        (
            "/edge_fwd.cpp",
            "include/geometry.h",
            HeaderUsage::DeclarationOnly,
        ),
        (
            "/edge_needs.cpp",
            "include/geometry.h",
            HeaderUsage::DeclarationOnly,
        ),
        (
            "/edge_diamond.cpp",
            "include/geometry.h",
            HeaderUsage::DeclarationOnly,
        ),
        (
            "/edge_diamond2.cpp",
            "include/geometry.h",
            HeaderUsage::DeclarationOnly,
        ),
        (
            "/main.cpp",
            "include/geometry.h",
            HeaderUsage::DeclarationOnly,
        ),
        (
            "/geometry.cpp",
            "include/geometry.h",
            HeaderUsage::DeclarationOnly,
        ),
        // Uses that require the definition.
        (
            "/edge_unused.cpp",
            "include/shapes.h",
            HeaderUsage::NeedsDefinition,
        ),
        (
            "/edge_needs.cpp",
            "include/fwd_class.h",
            HeaderUsage::NeedsDefinition,
        ),
        (
            "/edge_diamond.cpp",
            "include/diamond_a.h",
            HeaderUsage::NeedsDefinition,
        ),
        (
            "/edge_diamond2.cpp",
            "include/diamond_b.h",
            HeaderUsage::NeedsDefinition,
        ),
        (
            "/main.cpp",
            "include/shapes.h",
            HeaderUsage::NeedsDefinition,
        ),
        ("/main.cpp", "include/utils.h", HeaderUsage::NeedsDefinition),
        (
            "/shapes.cpp",
            "include/shapes.h",
            HeaderUsage::NeedsDefinition,
        ),
    ];

    for (unit_suffix, header_suffix, want) in expected {
        let unit = result
            .unit_analysis
            .iter()
            .find(|(id, _)| resolve(strings, **id).ends_with(unit_suffix))
            .unwrap_or_else(|| panic!("missing unit '{unit_suffix}'"))
            .1;
        let hid = header_id(strings, result, header_suffix)
            .unwrap_or_else(|| panic!("missing header '{header_suffix}'"));
        let got = unit.header_usage.get(&hid).unwrap_or_else(|| {
            panic!("unit '{unit_suffix}' did not classify header '{header_suffix}'")
        });
        assert_eq!(
            got, want,
            "unit '{unit_suffix}' header '{header_suffix}': expected {want:?}, got {got:?}"
        );
    }

    // shapes.cpp only includes shapes.h directly. geometry.h and vector3.h are
    // reached through it and must not be classified for this unit.
    let shapes = result
        .unit_analysis
        .iter()
        .find(|(id, _)| resolve(strings, **id).ends_with("/shapes.cpp"))
        .expect("shapes.cpp unit")
        .1;
    for suffix in ["include/geometry.h", "include/vector3.h"] {
        let hid = header_id(strings, result, suffix).expect("transitive header");
        assert!(
            !shapes.header_usage.contains_key(&hid),
            "'{suffix}' is transitive in shapes.cpp and must not be classified"
        );
    }
}

/// Every fixture build must expose the same CMake target graph.
fn assert_target_graph(dir: &Path, result: &AnalysisResult) {
    let graph = result
        .targets
        .as_ref()
        .unwrap_or_else(|| panic!("no CMake target data in {}", dir.display()));

    let names: Vec<&str> = graph.targets.iter().map(|t| t.name.as_str()).collect();
    assert!(names.contains(&"geometry_lib"), "targets: {names:?}");
    assert!(names.contains(&"shapely"), "targets: {names:?}");
    assert!(
        graph
            .edges
            .iter()
            .any(|e| e.from == "shapely" && e.to == "geometry_lib"),
        "expected a shapely -> geometry_lib edge, got: {:?}",
        graph
            .edges
            .iter()
            .map(|e| format!("{}->{}", e.from, e.to))
            .collect::<Vec<_>>()
    );
    assert_target_durations(dir, result);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn general_fixture_build_dirs_report_correct_data() {
    let dirs = fixture_build_dirs();
    if dirs.is_empty() {
        eprintln!(
            "SKIP: no fixture build dirs found; run tests/data/cpp-general-project/build-traces.sh --build-all"
        );
        return;
    }
    let jobs = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    for dir in &dirs {
        eprintln!("analyzing fixture build dir: {}", dir.display());
        let analysis =
            analyzer::analyze_build_dir(dir.to_str().unwrap(), jobs, &[], false, false).unwrap();
        assert_aggregation_invariants(&analysis.strings, &analysis.result);
        assert_fixture_structure(&analysis.strings, &analysis.result);
        assert_target_graph(dir, &analysis.result);
        let info = build_time_insight::build_info::extract_build_info(
            dir.to_str().expect("fixture path is valid UTF-8"),
        );
        assert_build_info_matches_dir(dir, &info);
    }
}

#[test]
fn general_fixture_report_round_trips() {
    let dirs = fixture_build_dirs();
    if dirs.is_empty() {
        eprintln!(
            "SKIP: no fixture build dirs found; run tests/data/cpp-general-project/build-traces.sh"
        );
        return;
    }

    for dir in &dirs {
        let analysis =
            analyzer::analyze_build_dir(dir.to_str().unwrap(), 4, &[], false, false).unwrap();

        // Serialize the analysis into a report, write it, and read it back.
        let report = Report::new(
            analysis.strings.clone(),
            BuildInfo::default(),
            analysis.result,
        );
        let path = std::env::temp_dir().join(format!(
            "bti_fixture_roundtrip_{}.bin",
            dir.file_name().unwrap().to_string_lossy()
        ));
        report::write_report(&report, &path).unwrap();
        let read = report::read_report(&path).unwrap();
        std::fs::remove_file(&path).ok();

        // The read-back report must satisfy the same invariants and structure.
        assert_aggregation_invariants(&read.strings, &read.analysis);
        assert_fixture_structure(&read.strings, &read.analysis);
        assert_target_graph(dir, &read.analysis);
    }
}

#[test]
fn general_fixture_header_usage_is_classified_correctly() {
    let dirs = fixture_build_dirs();
    let dirs: Vec<&PathBuf> = dirs
        .iter()
        .filter(|d| d.join("compile_commands.json").is_file())
        .collect();
    if dirs.is_empty() {
        eprintln!("SKIP: no fixture build dir with compile_commands.json");
        return;
    }

    dirs.par_iter().for_each(|dir| {
        eprintln!(
            "analyzing header usage in fixture build dir: {}",
            dir.display()
        );
        // One worker per build dir; header usage spawns one compiler per unit.
        let analysis =
            analyzer::analyze_build_dir(dir.to_str().unwrap(), 1, &[], true, false).unwrap();
        assert_header_usage(&analysis.strings, &analysis.result);
    });
}

#[test]
fn general_fixture_target_graph_is_correct() {
    let dirs = fixture_build_dirs();
    if dirs.is_empty() {
        eprintln!("SKIP: no fixture build dirs found");
        return;
    }

    for dir in &dirs {
        let analysis =
            analyzer::analyze_build_dir(dir.to_str().unwrap(), 4, &[], false, false).unwrap();
        assert_target_graph(dir, &analysis.result);
    }
}

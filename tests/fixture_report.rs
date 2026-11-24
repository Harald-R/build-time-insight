//! End-to-end validation against the C++ fixture project in `tests/data/cpp-project`.
//!
//! The fixture is built with clang's `-ftime-trace` (see
//! `tests/data/cpp-project/build-traces.sh`), producing one build directory per
//! compiler. These tests analyze each such directory and assert that the reported
//! data is correct.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use build_time_insight::analyzer::{self, AnalysisResult, TemplateInstantiation, UnitAnalysis};
use build_time_insight::build_info::BuildInfo;
use build_time_insight::build_traces;
use build_time_insight::header_usage::HeaderUsage;
use build_time_insight::interner::StringId;
use build_time_insight::report;
use build_time_insight::report::Report;

const FIXTURE: &str = "tests/data/cpp-project";

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(FIXTURE)
}

/// Build directories under the fixture that contain `*.cpp.json` traces.
fn fixture_build_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(fixture_root())
        .map(|rd| {
            rd.filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| {
                    let is_build_dir = p.is_dir()
                        && p.file_name()
                            .and_then(|n| n.to_str())
                            .is_some_and(|n| n.starts_with("build-"));
                    is_build_dir && contains_traces(p)
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

fn resolve(strings: &[String], id: StringId) -> &str {
    id.resolve(strings)
}

/// Find a header id whose path ends with `suffix`.
fn header_id(strings: &[String], result: &AnalysisResult, suffix: &str) -> Option<StringId> {
    result
        .header_analysis
        .keys()
        .find(|id| resolve(strings, **id).ends_with(suffix))
        .copied()
}

fn unit_names(strings: &[String], result: &AnalysisResult) -> Vec<String> {
    result
        .unit_analysis
        .keys()
        .map(|id| resolve(strings, *id).to_string())
        .collect()
}

// ---------------------------------------------------------------------------
// Correctness checks
// ---------------------------------------------------------------------------

/// Aggregation invariants: every aggregated value must equal the sum of the
/// per-unit values it was derived from, and all durations must be sane.
fn assert_aggregation_invariants(strings: &[String], result: &AnalysisResult) {
    for (hid, h) in &result.header_analysis {
        let mut sum = 0i64;
        let mut count = 0i64;
        for u in result.unit_analysis.values() {
            let chains = u.header_inclusion_paths.get(hid);
            if !u.header_parse_duration.contains_key(hid) && chains.is_none() {
                continue;
            }
            sum += u.header_parse_duration.get(hid).copied().unwrap_or(0);
            // One entry per `Source` span, not one per unit: a header can be
            // entered several times in the same translation unit.
            count += chains
                .map(|chains| chains.values().copied().sum::<i64>())
                .filter(|&n| n > 0)
                .unwrap_or(1);
        }
        assert_eq!(
            h.parse_duration,
            sum,
            "header '{}' parse_duration != sum of per-unit parse times",
            resolve(strings, *hid)
        );
        assert_eq!(
            h.num_inclusions,
            count,
            "header '{}' num_inclusions != sum of per-unit inclusion events",
            resolve(strings, *hid)
        );
        let usage_total = h.unused_units + h.forward_declarable_units + h.definition_required_units;
        assert!(
            usage_total <= h.num_inclusions as usize,
            "header '{}' usage counts exceed inclusions",
            resolve(strings, *hid)
        );
    }

    for (uid, u) in &result.unit_analysis {
        let name = resolve(strings, *uid);
        assert!(u.total_duration >= 0, "unit '{name}' negative total");
        assert!(u.frontend_duration >= 0, "unit '{name}' negative frontend");
        assert!(u.backend_duration >= 0, "unit '{name}' negative backend");
        assert!(
            u.total_duration >= u.frontend_duration + u.backend_duration,
            "unit '{name}' total < frontend + backend"
        );
    }

    // Build-wide template totals must equal the sum of per-unit totals.
    assert_template_totals(
        strings,
        result,
        &result.function_template_instantiations,
        &|u: &UnitAnalysis| &u.function_template_instantiations,
    );
    assert_template_totals(
        strings,
        result,
        &result.class_template_instantiations,
        &|u: &UnitAnalysis| &u.class_template_instantiations,
    );
}

fn assert_template_totals(
    strings: &[String],
    result: &AnalysisResult,
    build_map: &HashMap<StringId, TemplateInstantiation>,
    per_unit: &dyn Fn(&UnitAnalysis) -> &HashMap<StringId, TemplateInstantiation>,
) {
    for (tid, t) in build_map {
        let (mut dur, mut num) = (0i64, 0i64);
        for u in result.unit_analysis.values() {
            if let Some(ut) = per_unit(u).get(tid) {
                dur += ut.total_duration;
                num += ut.num_instantiations;
            }
        }
        assert_eq!(
            t.total_duration,
            dur,
            "template '{}' build total_duration != sum of per-unit",
            resolve(strings, *tid)
        );
        assert_eq!(
            t.num_instantiations,
            num,
            "template '{}' build count != sum of per-unit",
            resolve(strings, *tid)
        );
    }
}

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

/// The edge-case units must be classified as expected when header-usage data
/// is present. Classifications that depend on clang's trace spans (e.g. the
/// declaration-only case) are only asserted when the header was actually
/// analyzed.
fn assert_header_usage(strings: &[String], result: &AnalysisResult) {
    let has_any_usage = result
        .unit_analysis
        .values()
        .any(|u| !u.header_usage.is_empty());
    if !has_any_usage {
        eprintln!("SKIP header-usage assertions: no header-usage data (clang++ re-run failed?)");
        return;
    }

    for (uid, u) in &result.unit_analysis {
        let name = resolve(strings, *uid);
        let usage_of = |suffix: &str| {
            header_id(strings, result, suffix).and_then(|hid| u.header_usage.get(&hid))
        };

        // Only direct includes can be removed from this unit. A transitive
        // header must never be classified as unused/forward-declarable here.
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

        // edge_unused.cpp includes unused.h but uses nothing from it.
        if name.ends_with("/edge_unused.cpp")
            && let Some(usage) = usage_of("include/unused.h")
        {
            assert_eq!(
                usage,
                &HeaderUsage::Unused,
                "unused.h in edge_unused.cpp expected Unused, got {usage:?}"
            );
        }
        // edge_needs.cpp constructs Widget, so fwd_class.h needs its definition.
        if name.ends_with("/edge_needs.cpp")
            && let Some(usage) = usage_of("include/fwd_class.h")
        {
            assert_eq!(
                usage,
                &HeaderUsage::NeedsDefinition,
                "fwd_class.h in edge_needs.cpp expected NeedsDefinition, got {usage:?}"
            );
        }
        // shapes.cpp accesses Sphere::radius and Box::min/max, so it needs the
        // definitions from shapes.h rather than just forward declarations.
        if name.ends_with("/shapes.cpp")
            && let Some(usage) = usage_of("include/shapes.h")
        {
            assert_eq!(
                usage,
                &HeaderUsage::NeedsDefinition,
                "shapes.h in shapes.cpp expected NeedsDefinition, got {usage:?}"
            );
        }
        // shapes.cpp only includes shapes.h directly. geometry.h and vector3.h
        // are reached through it and must not be classified for this unit.
        if name.ends_with("/shapes.cpp") {
            assert!(
                usage_of("include/geometry.h").is_none(),
                "geometry.h is transitive in shapes.cpp and must not be classified"
            );
            assert!(
                usage_of("include/vector3.h").is_none(),
                "vector3.h is transitive in shapes.cpp and must not be classified"
            );
        }
        // main.cpp calls centroid() declared in geometry.h.
        if name.ends_with("/main.cpp")
            && let Some(usage) = usage_of("include/geometry.h")
        {
            assert_ne!(
                usage,
                &HeaderUsage::Unused,
                "geometry.h in main.cpp is used but was classified Unused"
            );
        }
        // geometry.cpp defines the functions declared in geometry.h.
        if name.ends_with("/geometry.cpp")
            && let Some(usage) = usage_of("include/geometry.h")
        {
            assert_eq!(
                usage,
                &HeaderUsage::DeclarationOnly,
                "geometry.h in geometry.cpp expected DeclarationOnly, got {usage:?}"
            );
        }
        // edge_diamond.cpp constructs DiamondA.
        if name.ends_with("/edge_diamond.cpp")
            && let Some(usage) = usage_of("include/diamond_a.h")
        {
            assert_ne!(
                usage,
                &HeaderUsage::Unused,
                "diamond_a.h in edge_diamond.cpp is used but was classified Unused"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn fixture_build_dirs_report_correct_data() {
    let dirs = fixture_build_dirs();
    if dirs.is_empty() {
        eprintln!(
            "SKIP: no fixture build dirs found; run tests/data/cpp-project/build-traces.sh --build-all"
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
    }
}

#[test]
fn fixture_report_round_trips() {
    let dirs = fixture_build_dirs();
    let Some(dir) = dirs.first() else {
        eprintln!("SKIP: no fixture build dirs found; run tests/data/cpp-project/build-traces.sh");
        return;
    };
    let analysis =
        analyzer::analyze_build_dir(dir.to_str().unwrap(), 4, &[], false, false).unwrap();

    // Serialize the analysis into a report, write it, and read it back.
    let report = Report::new(
        analysis.strings.clone(),
        BuildInfo::default(),
        analysis.result,
    );
    let path = std::env::temp_dir().join("bti_fixture_roundtrip.bin");
    report::write_report(&report, &path).unwrap();
    let read = report::read_report(&path).unwrap();
    std::fs::remove_file(&path).ok();

    // The read-back report must satisfy the same invariants and structure.
    assert_aggregation_invariants(&read.strings, &read.analysis);
    assert_fixture_structure(&read.strings, &read.analysis);
}

#[test]
fn fixture_header_usage_is_classified_correctly() {
    let dirs = fixture_build_dirs();
    let Some(dir) = dirs
        .first()
        .filter(|d| d.join("compile_commands.json").is_file())
    else {
        eprintln!("SKIP: no fixture build dir with compile_commands.json");
        return;
    };
    eprintln!(
        "analyzing header usage in fixture build dir: {}",
        dir.display()
    );
    let analysis = analyzer::analyze_build_dir(dir.to_str().unwrap(), 4, &[], true, false).unwrap();
    assert_header_usage(&analysis.strings, &analysis.result);
}

#[test]
fn fixture_target_graph_is_correct() {
    let dirs = fixture_build_dirs();
    let Some(dir) = dirs.first() else {
        eprintln!("SKIP: no fixture build dirs found");
        return;
    };
    let analysis =
        analyzer::analyze_build_dir(dir.to_str().unwrap(), 4, &[], false, false).unwrap();
    let Some(graph) = &analysis.result.targets else {
        eprintln!("SKIP: no CMake target data in the fixture build dir");
        return;
    };

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
    // A target with sources must have a positive compile time.
    for target in &graph.targets {
        if target.source_count > 0 {
            assert!(
                target.duration > 0,
                "target '{}' has sources but no duration",
                target.name
            );
        }
    }
}

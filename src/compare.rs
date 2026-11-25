//! Compares two build-time reports.
//!
//! Items are matched by their resolved names because the two reports intern
//! strings independently, so their `StringId`s are not comparable.

use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
    fmt,
};

use crate::analyzer::TemplateInstantiation;
use crate::interner::StringId;
use crate::report::Report;
use crate::summary::Seconds;

/// How an item's duration changed between two reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// The item exists only in the target report.
    Added,
    /// The item exists only in the baseline report.
    Removed,
    /// The item became slower in the target report.
    Slower,
    /// The item became faster in the target report.
    Faster,
    /// The item has the same duration in both reports.
    Unchanged,
}

/// One item's duration in the baseline and target reports.
///
/// A `None` duration means the item is absent from that report. Comparing
/// `Option`s instead of defaulting a missing item to zero keeps added and
/// removed items distinguishable from real duration changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurationDiff {
    /// Resolved item name.
    pub name: String,
    /// Total duration in the baseline report, in microseconds.
    pub base_micros: Option<i64>,
    /// Total duration in the target report, in microseconds.
    pub target_micros: Option<i64>,
}

impl DurationDiff {
    /// Creates a duration diff.
    pub fn new(
        name: impl Into<String>,
        base_micros: Option<i64>,
        target_micros: Option<i64>,
    ) -> Self {
        Self {
            name: name.into(),
            base_micros,
            target_micros,
        }
    }

    /// Returns how the item changed between the two reports.
    pub fn change(&self) -> Change {
        match (self.base_micros, self.target_micros) {
            (None, Some(_)) => Change::Added,
            (Some(_), None) => Change::Removed,
            (Some(base), Some(target)) if target > base => Change::Slower,
            (Some(base), Some(target)) if target < base => Change::Faster,
            _ => Change::Unchanged,
        }
    }

    /// Returns the duration change in microseconds (`target - base`), or
    /// `None` when the item is present in only one report.
    pub fn delta_micros(&self) -> Option<i64> {
        Some(self.target_micros? - self.base_micros?)
    }

    /// Returns the percentage change relative to the baseline duration.
    ///
    /// Returns `None` when the item is absent from one report or the baseline
    /// duration is zero, since a percentage is undefined in those cases.
    pub fn percent_change(&self) -> Option<f64> {
        let base = self.base_micros?;
        let delta = self.delta_micros()?;
        (base != 0).then(|| delta as f64 / base as f64 * 100.0)
    }

    /// Formats the change without the leading item name, so callers can align
    /// the name column.
    pub fn describe(&self) -> String {
        let seconds = Seconds::from_micros;
        match self.change() {
            Change::Added => {
                format!("(new) -> {}", seconds(self.target_micros.unwrap_or(0)))
            }
            Change::Removed => {
                format!("{} -> (removed)", seconds(self.base_micros.unwrap_or(0)))
            }
            Change::Unchanged => {
                format!("{} (unchanged)", seconds(self.base_micros.unwrap_or(0)))
            }
            Change::Slower | Change::Faster => {
                let base = self.base_micros.unwrap_or(0);
                let target = self.target_micros.unwrap_or(0);
                let delta = target - base;
                let percent = self
                    .percent_change()
                    .map(|percent| format!(" ({percent:+.1}%)"))
                    .unwrap_or_default();
                let label = if delta > 0 {
                    "regression"
                } else {
                    "improvement"
                };
                format!(
                    "{} -> {}  {}{percent}  {label}",
                    seconds(base),
                    seconds(target),
                    format_signed_micros(delta)
                )
            }
        }
    }
}

impl fmt::Display for DurationDiff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}  {}", self.name, self.describe())
    }
}

/// Formats a signed duration with an adaptive unit, keeping the sign outside
/// so negative values render as `-360.000 ms` rather than `-360000 us`.
fn format_signed_micros(micros: i64) -> String {
    let magnitude = Seconds::from_micros(micros.unsigned_abs() as i64);
    match micros.cmp(&0) {
        Ordering::Greater => format!("+{magnitude}"),
        Ordering::Less => format!("-{magnitude}"),
        Ordering::Equal => magnitude.to_string(),
    }
}

/// Compares the build-wide totals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TotalsDiff {
    /// Total compile duration.
    pub total: DurationDiff,
    /// Frontend duration.
    pub frontend: DurationDiff,
    /// Backend duration.
    pub backend: DurationDiff,
    /// Duration not attributed to the frontend or backend.
    pub other: DurationDiff,
    /// Duration spent instantiating templates.
    pub template_instantiation: DurationDiff,
}

impl TotalsDiff {
    fn new(base: &Report, target: &Report) -> Self {
        let base_totals = build_totals(base);
        let target_totals = build_totals(target);
        Self {
            total: DurationDiff::new("total", Some(base_totals.total), Some(target_totals.total)),
            frontend: DurationDiff::new(
                "frontend",
                Some(base_totals.frontend),
                Some(target_totals.frontend),
            ),
            backend: DurationDiff::new(
                "backend",
                Some(base_totals.backend),
                Some(target_totals.backend),
            ),
            other: DurationDiff::new("other", Some(base_totals.other), Some(target_totals.other)),
            template_instantiation: DurationDiff::new(
                "template instantiation",
                Some(base_totals.template_instantiation),
                Some(target_totals.template_instantiation),
            ),
        }
    }

    /// Returns the totals in display order.
    pub fn rows(&self) -> [&DurationDiff; 5] {
        [
            &self.total,
            &self.frontend,
            &self.backend,
            &self.other,
            &self.template_instantiation,
        ]
    }
}

/// A header's parse-time comparison, including how often it was included.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeaderDiff {
    /// Parsing time comparison. Its `name` is the header path.
    pub parse_duration: DurationDiff,
    /// Number of times the header was included in the baseline report.
    pub base_inclusions: Option<i64>,
    /// Number of times the header was included in the target report.
    pub target_inclusions: Option<i64>,
}

impl HeaderDiff {
    /// Returns the resolved header path.
    pub fn name(&self) -> &str {
        &self.parse_duration.name
    }

    /// Returns whether the parse time or the inclusion count changed.
    pub fn is_changed(&self) -> bool {
        self.parse_duration.change() != Change::Unchanged
            || self.base_inclusions != self.target_inclusions
    }

    /// Formats the change without the leading header name.
    pub fn describe(&self) -> String {
        let mut text = self.parse_duration.describe();
        if let (Some(base), Some(target)) = (self.base_inclusions, self.target_inclusions)
            && base != target
        {
            text.push_str(&format!("  inclusions {base} -> {target}"));
        }
        text
    }
}

impl fmt::Display for HeaderDiff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}  {}", self.name(), self.describe())
    }
}

/// A complete comparison of two reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReportDiff {
    /// Build-wide totals.
    pub totals: TotalsDiff,
    /// CMake target durations, when both reports contain target data.
    pub targets: Option<Vec<DurationDiff>>,
    /// Per-header parse time and inclusion counts.
    pub headers: Vec<HeaderDiff>,
    /// Function template instantiation durations.
    pub function_templates: Vec<DurationDiff>,
    /// Class template instantiation durations.
    pub class_templates: Vec<DurationDiff>,
    /// Per-translation-unit total durations.
    pub units: Vec<DurationDiff>,
}

/// Compares the build totals, targets, headers, templates, and units of two
/// reports.
pub fn report_diff(base: &Report, target: &Report) -> ReportDiff {
    ReportDiff {
        totals: TotalsDiff::new(base, target),
        targets: target_diffs(base, target),
        headers: header_diffs(base, target),
        function_templates: template_diffs(
            base,
            target,
            &base.analysis.function_template_instantiations,
            &target.analysis.function_template_instantiations,
        ),
        class_templates: template_diffs(
            base,
            target,
            &base.analysis.class_template_instantiations,
            &target.analysis.class_template_instantiations,
        ),
        units: unit_duration_diffs(base, target),
    }
}

impl ReportDiff {
    /// Prints the comparison to the terminal, one section at a time.
    pub fn print(&self) {
        println!("Report comparison (base -> target)");
        self.print_totals();
        self.print_targets();
        self.print_headers();
        self.print_templates();
        self.print_units();
    }

    fn print_totals(&self) {
        print_section("Build totals (base -> target)");
        let rows = self.totals.rows();
        let width = rows.iter().map(|row| row.name.len()).max().unwrap_or(0);
        for row in rows {
            println!("  {:<width$}  {}", row.name, row.describe());
        }
    }

    fn print_targets(&self) {
        print_section("Targets (base -> target)");
        match &self.targets {
            None => println!("  (target data unavailable in one or both reports)"),
            Some(targets) => print_changed_rows(targets, "(no target duration changes)"),
        }
    }

    fn print_headers(&self) {
        print_section("Headers (base -> target)");
        let changed: Vec<&HeaderDiff> = self
            .headers
            .iter()
            .filter(|header| header.is_changed())
            .collect();
        if changed.is_empty() {
            println!("  (no header parse-time or inclusion changes)");
            return;
        }
        let width = changed
            .iter()
            .map(|header| header.name().chars().count())
            .max()
            .unwrap_or(0);
        for header in changed {
            println!("  {:<width$}  {}", header.name(), header.describe());
        }
    }

    fn print_templates(&self) {
        print_section("Templates (base -> target)");
        let mut rows: Vec<(&str, &DurationDiff)> = Vec::new();
        rows.extend(
            self.function_templates
                .iter()
                .filter(|template| template.change() != Change::Unchanged)
                .map(|template| ("function", template)),
        );
        rows.extend(
            self.class_templates
                .iter()
                .filter(|template| template.change() != Change::Unchanged)
                .map(|template| ("class", template)),
        );
        if rows.is_empty() {
            println!("  (no template instantiation changes)");
            return;
        }
        let width = rows
            .iter()
            .map(|(_, template)| template.name.chars().count())
            .max()
            .unwrap_or(0);
        for (kind, template) in rows {
            println!(
                "  {kind:<8}  {:<width$}  {}",
                template.name,
                template.describe()
            );
        }
    }

    fn print_units(&self) {
        print_section("Per-unit total durations (base -> target)");
        if self.units.is_empty() {
            println!("  (no translation units in either report)");
            return;
        }
        let width = self
            .units
            .iter()
            .map(|unit| unit.name.chars().count())
            .max()
            .unwrap_or(0);
        for unit in &self.units {
            println!("  {:<width$}  {}", unit.name, unit.describe());
        }
    }
}

fn print_section(title: &str) {
    println!();
    println!("{title}");
    println!("{}", "─".repeat(title.chars().count()));
}

fn print_changed_rows(rows: &[DurationDiff], empty: &str) {
    let changed: Vec<&DurationDiff> = rows
        .iter()
        .filter(|row| row.change() != Change::Unchanged)
        .collect();
    if changed.is_empty() {
        println!("  {empty}");
        return;
    }
    let width = changed
        .iter()
        .map(|row| row.name.chars().count())
        .max()
        .unwrap_or(0);
    for row in changed {
        println!("  {:<width$}  {}", row.name, row.describe());
    }
}

/// Per-unit total duration differences (`target - base`), covering every unit
/// found in either report.
///
/// Rows with a comparable duration are sorted from the largest regression to
/// the largest improvement, then units that exist in only one report follow
/// in name order. Units are matched by resolved name because the two reports
/// intern strings independently and their ids are not comparable.
pub fn unit_duration_diffs(base: &Report, target: &Report) -> Vec<DurationDiff> {
    let mut diffs: Vec<DurationDiff> =
        Vec::with_capacity(base.analysis.unit_analysis.len() + target.analysis.unit_analysis.len());

    let mut seen: HashSet<&str> = HashSet::with_capacity(target.analysis.unit_analysis.len());
    for (unit, analysis) in &target.analysis.unit_analysis {
        let unit_name = target.resolve(*unit);
        seen.insert(unit_name);
        let base_micros = base
            .id_of(unit_name)
            .and_then(|id| base.analysis.unit_analysis.get(&id))
            .map(|analysis| analysis.total_duration);
        diffs.push(DurationDiff::new(
            unit_name,
            base_micros,
            Some(analysis.total_duration),
        ));
    }

    for (unit, analysis) in &base.analysis.unit_analysis {
        let unit_name = base.resolve(*unit);
        if seen.contains(unit_name) {
            continue;
        }
        diffs.push(DurationDiff::new(
            unit_name,
            Some(analysis.total_duration),
            None,
        ));
    }

    sort_duration_diffs(&mut diffs);
    diffs
}

/// Per-header parse-time differences, covering every header found in either
/// report. Inclusion counts are compared alongside the parse time.
fn header_diffs(base: &Report, target: &Report) -> Vec<HeaderDiff> {
    let mut diffs: Vec<HeaderDiff> = Vec::with_capacity(
        base.analysis.header_analysis.len() + target.analysis.header_analysis.len(),
    );

    let mut seen: HashSet<&str> = HashSet::with_capacity(target.analysis.header_analysis.len());
    for (header, analysis) in &target.analysis.header_analysis {
        let header_name = target.resolve(*header);
        seen.insert(header_name);
        let base_header = base
            .id_of(header_name)
            .and_then(|id| base.analysis.header_analysis.get(&id));
        diffs.push(HeaderDiff {
            parse_duration: DurationDiff::new(
                header_name,
                base_header.map(|header| header.parse_duration),
                Some(analysis.parse_duration),
            ),
            base_inclusions: base_header.map(|header| header.num_inclusions),
            target_inclusions: Some(analysis.num_inclusions),
        });
    }

    for (header, analysis) in &base.analysis.header_analysis {
        let header_name = base.resolve(*header);
        if seen.contains(header_name) {
            continue;
        }
        diffs.push(HeaderDiff {
            parse_duration: DurationDiff::new(header_name, Some(analysis.parse_duration), None),
            base_inclusions: Some(analysis.num_inclusions),
            target_inclusions: None,
        });
    }

    diffs.sort_by(|a, b| compare_duration_diffs(&a.parse_duration, &b.parse_duration));
    diffs
}

/// Per-template instantiation differences for one template kind.
fn template_diffs(
    base: &Report,
    target: &Report,
    base_templates: &HashMap<StringId, TemplateInstantiation>,
    target_templates: &HashMap<StringId, TemplateInstantiation>,
) -> Vec<DurationDiff> {
    let mut diffs: Vec<DurationDiff> =
        Vec::with_capacity(base_templates.len() + target_templates.len());

    let mut seen: HashSet<&str> = HashSet::with_capacity(target_templates.len());
    for (template, instantiation) in target_templates {
        let template_name = target.resolve(*template);
        seen.insert(template_name);
        let base_micros = base
            .id_of(template_name)
            .and_then(|id| base_templates.get(&id))
            .map(|instantiation| instantiation.total_duration);
        diffs.push(DurationDiff::new(
            template_name,
            base_micros,
            Some(instantiation.total_duration),
        ));
    }

    for (template, instantiation) in base_templates {
        let template_name = base.resolve(*template);
        if seen.contains(template_name) {
            continue;
        }
        diffs.push(DurationDiff::new(
            template_name,
            Some(instantiation.total_duration),
            None,
        ));
    }

    sort_duration_diffs(&mut diffs);
    diffs
}

/// Per-target duration differences. Returns `None` when either report lacks
/// target data, since comparing against an absent target graph would report
/// every target as added or removed.
fn target_diffs(base: &Report, target: &Report) -> Option<Vec<DurationDiff>> {
    let base_graph = base.analysis.targets.as_ref()?;
    let target_graph = target.analysis.targets.as_ref()?;

    let base_targets: HashMap<&str, i64> = base_graph
        .targets
        .iter()
        .map(|target| (target.name.as_str(), target.duration))
        .collect();
    let target_targets: HashMap<&str, i64> = target_graph
        .targets
        .iter()
        .map(|target| (target.name.as_str(), target.duration))
        .collect();

    let mut diffs: Vec<DurationDiff> =
        Vec::with_capacity(base_targets.len() + target_targets.len());
    let mut seen: HashSet<&str> = HashSet::with_capacity(target_targets.len());
    for (name, duration) in &target_targets {
        seen.insert(*name);
        diffs.push(DurationDiff::new(
            *name,
            base_targets.get(name).copied(),
            Some(*duration),
        ));
    }
    for (name, duration) in &base_targets {
        if seen.contains(name) {
            continue;
        }
        diffs.push(DurationDiff::new(*name, Some(*duration), None));
    }

    sort_duration_diffs(&mut diffs);
    Some(diffs)
}

/// Build-wide duration totals, in microseconds.
#[derive(Debug, Default, Clone, Copy)]
struct BuildTotals {
    total: i64,
    frontend: i64,
    backend: i64,
    other: i64,
    template_instantiation: i64,
}

fn build_totals(report: &Report) -> BuildTotals {
    let mut totals = BuildTotals::default();
    for unit in report.analysis.unit_analysis.values() {
        totals.total += unit.total_duration;
        totals.frontend += unit.frontend_duration;
        totals.backend += unit.backend_duration;
        totals.template_instantiation += unit.template_instantiation_duration;
    }
    // Match `ReportSummary::totals`, which derives `other` from the aggregate
    // phases rather than summing each unit's clamped remainder.
    totals.other = (totals.total - totals.frontend - totals.backend).max(0);
    totals
}

fn compare_duration_diffs(a: &DurationDiff, b: &DurationDiff) -> Ordering {
    match (a.delta_micros(), b.delta_micros()) {
        (Some(a_delta), Some(b_delta)) => b_delta.cmp(&a_delta).then_with(|| a.name.cmp(&b.name)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => a.name.cmp(&b.name),
    }
}

fn sort_duration_diffs(diffs: &mut [DurationDiff]) {
    diffs.sort_by(compare_duration_diffs);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::{AnalysisResult, HeaderAnalysis, UnitAnalysis};
    use crate::build_info::BuildInfo;
    use crate::targets::{TargetGraph, TargetNode};
    use std::collections::{BTreeMap, HashMap};

    fn id_of(strings: &[String], s: &str) -> StringId {
        StringId::from(strings.binary_search_by(|x| x.as_str().cmp(s)).unwrap() as u32)
    }

    fn empty_unit(total_duration: i64) -> UnitAnalysis {
        UnitAnalysis {
            total_duration,
            frontend_duration: 0,
            backend_duration: 0,
            template_instantiation_duration: 0,
            header_parse_duration: HashMap::new(),
            header_inclusion_paths: BTreeMap::new(),
            header_usage: HashMap::new(),
            function_template_instantiations: HashMap::new(),
            class_template_instantiations: HashMap::new(),
        }
    }

    fn unit_of(
        total_duration: i64,
        frontend_duration: i64,
        backend_duration: i64,
        template_instantiation_duration: i64,
    ) -> UnitAnalysis {
        let mut unit = empty_unit(total_duration);
        unit.frontend_duration = frontend_duration;
        unit.backend_duration = backend_duration;
        unit.template_instantiation_duration = template_instantiation_duration;
        unit
    }

    fn report_with_units(units: Vec<(&str, UnitAnalysis)>) -> Report {
        let mut strings: Vec<String> = units.iter().map(|(name, _)| (*name).to_string()).collect();
        strings.sort();
        strings.dedup();
        let unit_analysis = units
            .into_iter()
            .map(|(name, unit)| (id_of(&strings, name), unit))
            .collect();
        Report::new(
            strings,
            BuildInfo::default(),
            AnalysisResult::from(unit_analysis),
        )
    }

    fn report_with_unit_durations(durations: &[(&str, i64)]) -> Report {
        report_with_units(
            durations
                .iter()
                .map(|(name, duration)| (*name, empty_unit(*duration)))
                .collect(),
        )
    }

    fn report_with_headers(entries: &[(&str, i64, i64)]) -> Report {
        let mut strings: Vec<String> = entries
            .iter()
            .map(|(name, _, _)| name.to_string())
            .collect();
        strings.sort();
        strings.dedup();
        let mut header_analysis = HashMap::new();
        for (name, parse_duration, num_inclusions) in entries {
            header_analysis.insert(
                id_of(&strings, name),
                HeaderAnalysis {
                    parse_duration: *parse_duration,
                    num_inclusions: *num_inclusions,
                    unused_units: 0,
                    forward_declarable_units: 0,
                    definition_required_units: 0,
                    inclusion_paths: BTreeMap::new(),
                },
            );
        }
        Report::new(
            strings,
            BuildInfo::default(),
            AnalysisResult {
                unit_analysis: HashMap::new(),
                header_analysis,
                function_template_instantiations: HashMap::new(),
                class_template_instantiations: HashMap::new(),
                targets: None,
            },
        )
    }

    fn report_with_templates(functions: &[(&str, i64)], classes: &[(&str, i64)]) -> Report {
        let mut strings: Vec<String> = functions
            .iter()
            .chain(classes.iter())
            .map(|(name, _)| name.to_string())
            .collect();
        strings.sort();
        strings.dedup();
        let function_template_instantiations = functions
            .iter()
            .map(|(name, duration)| (id_of(&strings, name), TemplateInstantiation::new(*duration)))
            .collect();
        let class_template_instantiations = classes
            .iter()
            .map(|(name, duration)| (id_of(&strings, name), TemplateInstantiation::new(*duration)))
            .collect();
        Report::new(
            strings,
            BuildInfo::default(),
            AnalysisResult {
                unit_analysis: HashMap::new(),
                header_analysis: HashMap::new(),
                function_template_instantiations,
                class_template_instantiations,
                targets: None,
            },
        )
    }

    fn report_with_targets(targets: &[(&str, i64)]) -> Report {
        let mut report = report_with_unit_durations(&[]);
        report.analysis.targets = Some(TargetGraph {
            targets: targets
                .iter()
                .map(|(name, duration)| TargetNode {
                    name: name.to_string(),
                    kind: "executable".to_string(),
                    dir: String::new(),
                    duration: *duration,
                    source_count: 1,
                })
                .collect(),
            edges: Vec::new(),
        });
        report
    }

    #[test]
    fn unit_duration_diffs_orders_by_regression_and_tracks_changes() {
        let base = report_with_unit_durations(&[("a.cpp", 100), ("b.cpp", 200), ("gone.cpp", 40)]);
        let target = report_with_unit_durations(&[("a.cpp", 160), ("b.cpp", 180), ("new.cpp", 50)]);

        let diffs = unit_duration_diffs(&base, &target);

        let summary: Vec<(&str, Option<i64>, Option<i64>, Change)> = diffs
            .iter()
            .map(|diff| {
                (
                    diff.name.as_str(),
                    diff.base_micros,
                    diff.target_micros,
                    diff.change(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                ("a.cpp", Some(100), Some(160), Change::Slower),
                ("b.cpp", Some(200), Some(180), Change::Faster),
                ("gone.cpp", Some(40), None, Change::Removed),
                ("new.cpp", None, Some(50), Change::Added),
            ]
        );

        assert_eq!(diffs[0].delta_micros(), Some(60));
        assert_eq!(diffs[0].percent_change(), Some(60.0));
        assert_eq!(diffs[1].delta_micros(), Some(-20));
        // Added and removed units have no comparable delta.
        assert_eq!(diffs[2].delta_micros(), None);
        assert_eq!(diffs[3].delta_micros(), None);
    }

    #[test]
    fn duration_diff_display_shows_base_target_and_change() {
        let slower = DurationDiff::new("src/a.cpp", Some(2_100_000), Some(2_460_000));
        assert_eq!(
            slower.to_string(),
            "src/a.cpp  2.100 s -> 2.460 s  +360.000 ms (+17.1%)  regression"
        );

        let faster = DurationDiff::new("src/b.cpp", Some(1_800_000), Some(1_500_000));
        assert_eq!(
            faster.to_string(),
            "src/b.cpp  1.800 s -> 1.500 s  -300.000 ms (-16.7%)  improvement"
        );

        let added = DurationDiff::new("src/new.cpp", None, Some(50_000));
        assert_eq!(added.to_string(), "src/new.cpp  (new) -> 50.000 ms");

        let removed = DurationDiff::new("src/old.cpp", Some(75_000), None);
        assert_eq!(removed.to_string(), "src/old.cpp  75.000 ms -> (removed)");

        let unchanged = DurationDiff::new("src/same.cpp", Some(100), Some(100));
        assert_eq!(unchanged.to_string(), "src/same.cpp  100 us (unchanged)");
    }

    #[test]
    fn totals_diff_compares_each_phase() {
        let base = report_with_units(vec![
            ("a.cpp", unit_of(1000, 500, 300, 200)),
            ("b.cpp", unit_of(500, 300, 100, 50)),
        ]);
        let target = report_with_units(vec![
            ("a.cpp", unit_of(1200, 600, 300, 250)),
            ("b.cpp", unit_of(500, 300, 100, 50)),
        ]);

        let diff = report_diff(&base, &target);

        assert_eq!(diff.totals.total.base_micros, Some(1500));
        assert_eq!(diff.totals.total.target_micros, Some(1700));
        assert_eq!(diff.totals.total.delta_micros(), Some(200));
        assert_eq!(diff.totals.frontend.delta_micros(), Some(100));
        assert_eq!(diff.totals.backend.delta_micros(), Some(0));
        assert_eq!(diff.totals.other.delta_micros(), Some(100));
        assert_eq!(diff.totals.template_instantiation.delta_micros(), Some(50));
        assert_eq!(diff.totals.rows().len(), 5);
    }

    #[test]
    fn header_diffs_track_parse_time_and_inclusions() {
        let base = report_with_headers(&[
            ("slow.h", 100, 2),
            ("inclusions.h", 50, 1),
            ("gone.h", 10, 1),
        ]);
        let target = report_with_headers(&[
            ("slow.h", 160, 3),
            ("inclusions.h", 50, 4),
            ("new.h", 20, 1),
        ]);

        let diff = report_diff(&base, &target);

        assert_eq!(diff.headers[0].name(), "slow.h");
        assert_eq!(diff.headers[0].parse_duration.delta_micros(), Some(60));
        assert_eq!(diff.headers[0].base_inclusions, Some(2));
        assert_eq!(diff.headers[0].target_inclusions, Some(3));
        assert!(diff.headers[0].describe().contains("inclusions 2 -> 3"));

        // An unchanged parse time with a changed inclusion count is still a
        // change and must survive filtering.
        let inclusions = diff
            .headers
            .iter()
            .find(|header| header.name() == "inclusions.h")
            .unwrap();
        assert!(inclusions.is_changed());
        assert_eq!(inclusions.parse_duration.delta_micros(), Some(0));
        assert!(inclusions.describe().contains("inclusions 1 -> 4"));

        let gone = diff
            .headers
            .iter()
            .find(|header| header.name() == "gone.h")
            .unwrap();
        assert_eq!(gone.parse_duration.change(), Change::Removed);
        assert!(gone.is_changed());

        let new = diff
            .headers
            .iter()
            .find(|header| header.name() == "new.h")
            .unwrap();
        assert_eq!(new.parse_duration.change(), Change::Added);
    }

    #[test]
    fn template_diffs_keep_function_and_class_sections_separate() {
        let base = report_with_templates(&[("f<int>", 100), ("gone_fn", 10)], &[("C<int>", 200)]);
        let target = report_with_templates(&[("f<int>", 150), ("new_fn", 20)], &[("C<int>", 180)]);

        let diff = report_diff(&base, &target);

        assert_eq!(diff.function_templates.len(), 3);
        assert_eq!(diff.function_templates[0].name, "f<int>");
        assert_eq!(diff.function_templates[0].delta_micros(), Some(50));
        assert_eq!(diff.function_templates[1].name, "gone_fn");
        assert_eq!(diff.function_templates[1].change(), Change::Removed);
        assert_eq!(diff.function_templates[2].name, "new_fn");
        assert_eq!(diff.function_templates[2].change(), Change::Added);

        assert_eq!(diff.class_templates.len(), 1);
        assert_eq!(diff.class_templates[0].name, "C<int>");
        assert_eq!(diff.class_templates[0].delta_micros(), Some(-20));
    }

    #[test]
    fn target_diffs_require_target_data_in_both_reports() {
        let base = report_with_targets(&[("lib", 1000), ("app", 500)]);
        let target = report_with_targets(&[("lib", 1200), ("app", 500), ("tests", 100)]);

        let diff = report_diff(&base, &target);
        let targets = diff.targets.expect("both reports have target data");
        assert_eq!(targets.len(), 3);
        assert_eq!(targets[0].name, "lib");
        assert_eq!(targets[0].delta_micros(), Some(200));
        assert_eq!(targets[1].name, "app");
        assert_eq!(targets[1].change(), Change::Unchanged);
        assert_eq!(targets[2].name, "tests");
        assert_eq!(targets[2].change(), Change::Added);

        let no_targets = report_with_unit_durations(&[]);
        assert!(report_diff(&base, &no_targets).targets.is_none());
    }
}

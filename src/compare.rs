//! Compares two build-time reports.
//!
//! Units are matched by their resolved names because the two reports intern
//! strings independently, so their `StringId`s are not comparable.

use std::{cmp::Ordering, collections::HashSet, fmt};

use crate::report::Report;
use crate::summary::Seconds;

/// How a translation unit's duration changed between two reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitChange {
    /// The unit exists only in the target report.
    Added,
    /// The unit exists only in the baseline report.
    Removed,
    /// The unit became slower in the target report.
    Slower,
    /// The unit became faster in the target report.
    Faster,
    /// The unit has the same duration in both reports.
    Unchanged,
}

/// One translation unit's duration in the baseline and target reports.
///
/// A `None` duration means the unit is absent from that report. Comparing
/// `Option`s instead of defaulting a missing unit to zero keeps added and
/// removed units distinguishable from real duration changes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitDurationDiff {
    /// Resolved translation unit name.
    pub unit: String,
    /// Total duration in the baseline report, in microseconds.
    pub base_micros: Option<i64>,
    /// Total duration in the target report, in microseconds.
    pub target_micros: Option<i64>,
}

impl UnitDurationDiff {
    /// Returns how the unit changed between the two reports.
    pub fn change(&self) -> UnitChange {
        match (self.base_micros, self.target_micros) {
            (None, Some(_)) => UnitChange::Added,
            (Some(_), None) => UnitChange::Removed,
            (Some(base), Some(target)) if target > base => UnitChange::Slower,
            (Some(base), Some(target)) if target < base => UnitChange::Faster,
            _ => UnitChange::Unchanged,
        }
    }

    /// Returns the duration change in microseconds (`target - base`), or
    /// `None` when the unit is present in only one report.
    pub fn delta_micros(&self) -> Option<i64> {
        Some(self.target_micros? - self.base_micros?)
    }

    /// Returns the percentage change relative to the baseline duration.
    ///
    /// Returns `None` when the unit is absent from one report or the baseline
    /// duration is zero, since a percentage is undefined in those cases.
    pub fn percent_change(&self) -> Option<f64> {
        let base = self.base_micros?;
        let delta = self.delta_micros()?;
        (base != 0).then(|| delta as f64 / base as f64 * 100.0)
    }

    /// Formats the change without the leading unit name, so callers can align
    /// the unit column.
    pub fn describe(&self) -> String {
        let seconds = Seconds::from_micros;
        match self.change() {
            UnitChange::Added => {
                format!("(new) -> {}", seconds(self.target_micros.unwrap_or(0)))
            }
            UnitChange::Removed => {
                format!("{} -> (removed)", seconds(self.base_micros.unwrap_or(0)))
            }
            UnitChange::Unchanged => {
                format!("{} (unchanged)", seconds(self.base_micros.unwrap_or(0)))
            }
            UnitChange::Slower | UnitChange::Faster => {
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

impl fmt::Display for UnitDurationDiff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}  {}", self.unit, self.describe())
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

/// Per-unit total duration differences (`target - base`), covering every unit
/// found in either report.
///
/// Rows with a comparable duration are sorted from the largest regression to
/// the largest improvement, then units that exist in only one report follow
/// in name order. Units are matched by resolved name because the two reports
/// intern strings independently and their ids are not comparable.
pub fn unit_duration_diffs(base: &Report, target: &Report) -> Vec<UnitDurationDiff> {
    let mut diffs: Vec<UnitDurationDiff> =
        Vec::with_capacity(base.analysis.unit_analysis.len() + target.analysis.unit_analysis.len());

    let mut seen: HashSet<&str> = HashSet::with_capacity(target.analysis.unit_analysis.len());
    for (unit, analysis) in &target.analysis.unit_analysis {
        let unit_name = target.resolve(*unit);
        seen.insert(unit_name);
        let base_micros = base
            .id_of(unit_name)
            .and_then(|id| base.analysis.unit_analysis.get(&id))
            .map(|analysis| analysis.total_duration);
        diffs.push(UnitDurationDiff {
            unit: unit_name.to_string(),
            base_micros,
            target_micros: Some(analysis.total_duration),
        });
    }

    for (unit, analysis) in &base.analysis.unit_analysis {
        let unit_name = base.resolve(*unit);
        if seen.contains(unit_name) {
            continue;
        }
        diffs.push(UnitDurationDiff {
            unit: unit_name.to_string(),
            base_micros: Some(analysis.total_duration),
            target_micros: None,
        });
    }

    diffs.sort_by(|a, b| match (a.delta_micros(), b.delta_micros()) {
        (Some(a_delta), Some(b_delta)) => b_delta.cmp(&a_delta).then_with(|| a.unit.cmp(&b.unit)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => a.unit.cmp(&b.unit),
    });
    diffs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::{AnalysisResult, UnitAnalysis};
    use crate::build_info::BuildInfo;
    use crate::interner::StringId;
    use std::collections::{BTreeMap, HashMap};

    fn report_with_unit_durations(durations: &[(&str, i64)]) -> Report {
        let mut strings: Vec<String> = durations.iter().map(|(n, _)| (*n).to_string()).collect();
        strings.sort();
        strings.dedup();
        let id_of = |s: &str| {
            StringId::from(strings.binary_search_by(|x| x.as_str().cmp(s)).unwrap() as u32)
        };
        let mut units = HashMap::new();
        for (name, duration) in durations {
            units.insert(
                id_of(name),
                UnitAnalysis {
                    total_duration: *duration,
                    frontend_duration: 0,
                    backend_duration: 0,
                    template_instantiation_duration: 0,
                    header_parse_duration: HashMap::new(),
                    header_inclusion_paths: BTreeMap::new(),
                    header_usage: HashMap::new(),
                    function_template_instantiations: HashMap::new(),
                    class_template_instantiations: HashMap::new(),
                },
            );
        }
        Report::new(strings, BuildInfo::default(), AnalysisResult::from(units))
    }

    #[test]
    fn unit_duration_diffs_orders_by_regression_and_tracks_changes() {
        let base = report_with_unit_durations(&[("a.cpp", 100), ("b.cpp", 200), ("gone.cpp", 40)]);
        let target = report_with_unit_durations(&[("a.cpp", 160), ("b.cpp", 180), ("new.cpp", 50)]);

        let diffs = unit_duration_diffs(&base, &target);

        let summary: Vec<(&str, Option<i64>, Option<i64>, UnitChange)> = diffs
            .iter()
            .map(|diff| {
                (
                    diff.unit.as_str(),
                    diff.base_micros,
                    diff.target_micros,
                    diff.change(),
                )
            })
            .collect();
        assert_eq!(
            summary,
            vec![
                ("a.cpp", Some(100), Some(160), UnitChange::Slower),
                ("b.cpp", Some(200), Some(180), UnitChange::Faster),
                ("gone.cpp", Some(40), None, UnitChange::Removed),
                ("new.cpp", None, Some(50), UnitChange::Added),
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
    fn unit_duration_diff_display_shows_base_target_and_change() {
        let slower = UnitDurationDiff {
            unit: "src/a.cpp".to_string(),
            base_micros: Some(2_100_000),
            target_micros: Some(2_460_000),
        };
        assert_eq!(
            slower.to_string(),
            "src/a.cpp  2.100 s -> 2.460 s  +360.000 ms (+17.1%)  regression"
        );

        let faster = UnitDurationDiff {
            unit: "src/b.cpp".to_string(),
            base_micros: Some(1_800_000),
            target_micros: Some(1_500_000),
        };
        assert_eq!(
            faster.to_string(),
            "src/b.cpp  1.800 s -> 1.500 s  -300.000 ms (-16.7%)  improvement"
        );

        let added = UnitDurationDiff {
            unit: "src/new.cpp".to_string(),
            base_micros: None,
            target_micros: Some(50_000),
        };
        assert_eq!(added.to_string(), "src/new.cpp  (new) -> 50.000 ms");

        let removed = UnitDurationDiff {
            unit: "src/old.cpp".to_string(),
            base_micros: Some(75_000),
            target_micros: None,
        };
        assert_eq!(removed.to_string(), "src/old.cpp  75.000 ms -> (removed)");

        let unchanged = UnitDurationDiff {
            unit: "src/same.cpp".to_string(),
            base_micros: Some(100),
            target_micros: Some(100),
        };
        assert_eq!(unchanged.to_string(), "src/same.cpp  100 us (unchanged)");
    }
}

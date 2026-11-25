//! YAML representation of a report comparison.
//!
//! The YAML document is the canonical, machine-readable format. It stores the
//! baseline and target durations plus derived values for convenience. Derived
//! values are recomputed when the document is read back, so a hand-edited
//! `change`, `delta_us`, or `percent` cannot make the rendered text
//! inconsistent.

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::{Change, DurationDiff, HeaderDiff, ReportDiff, Thresholds, TotalsDiff};

/// The schema version written and read by this build.
pub const SCHEMA_VERSION: u32 = 1;

/// A failure to read or write a YAML comparison.
#[derive(Debug, Error)]
pub enum YamlError {
    /// Indicates a YAML encoding or decoding error.
    #[error("YAML error: {0}")]
    Yaml(#[from] serde_yaml_ng::Error),
    /// Indicates a document written by an unsupported schema version.
    #[error(
        "unsupported comparison schema version {found}; this build supports version {supported}"
    )]
    UnsupportedVersion { found: u32, supported: u32 },
}

/// A comparison read from YAML, including the thresholds stored in it.
#[derive(Debug, Clone, PartialEq)]
pub struct ComparisonDocument {
    /// The comparison itself.
    pub diff: ReportDiff,
    /// Thresholds stored with the document.
    pub thresholds: Thresholds,
}

/// Serializes a comparison to YAML.
pub fn to_string(diff: &ReportDiff, thresholds: &Thresholds) -> Result<String, YamlError> {
    Ok(serde_yaml_ng::to_string(&ReportDiffView::from_diff(
        diff, thresholds,
    ))?)
}

/// Deserializes a comparison from YAML.
pub fn from_str(yaml: &str) -> Result<ComparisonDocument, YamlError> {
    let view: ReportDiffView = serde_yaml_ng::from_str(yaml)?;
    if view.schema_version != SCHEMA_VERSION {
        return Err(YamlError::UnsupportedVersion {
            found: view.schema_version,
            supported: SCHEMA_VERSION,
        });
    }
    Ok(view.into_document())
}

/// Derived duration fields shared by every row.
#[derive(Debug, Serialize, Deserialize)]
struct MetricView {
    base_us: Option<i64>,
    target_us: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    delta_us: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    percent: Option<f64>,
    /// Derived from `base_us` and `target_us` when read back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    change: Option<Change>,
}

impl MetricView {
    fn from_diff(diff: &DurationDiff) -> Self {
        Self {
            base_us: diff.base_micros,
            target_us: diff.target_micros,
            delta_us: diff.delta_micros(),
            percent: diff.percent_change(),
            change: Some(diff.change()),
        }
    }

    fn into_diff(self, name: impl Into<String>) -> DurationDiff {
        DurationDiff::new(name, self.base_us, self.target_us)
    }
}

/// A named duration row.
#[derive(Debug, Serialize, Deserialize)]
struct NamedMetricView {
    name: String,
    #[serde(flatten)]
    metric: MetricView,
}

impl NamedMetricView {
    fn from_diff(diff: &DurationDiff) -> Self {
        Self {
            name: diff.name.clone(),
            metric: MetricView::from_diff(diff),
        }
    }

    fn into_diff(self) -> DurationDiff {
        self.metric.into_diff(self.name)
    }
}

/// Inclusion counts for a header.
#[derive(Debug, Default, Serialize, Deserialize)]
struct InclusionsView {
    #[serde(default)]
    base: Option<i64>,
    #[serde(default)]
    target: Option<i64>,
    /// Derived from `base` and `target` when read back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    delta: Option<i64>,
}

impl InclusionsView {
    fn from_inclusions(base: Option<i64>, target: Option<i64>) -> Self {
        Self {
            base,
            target,
            delta: match (base, target) {
                (Some(base), Some(target)) => Some(target - base),
                _ => None,
            },
        }
    }
}

/// A header row.
#[derive(Debug, Serialize, Deserialize)]
struct HeaderView {
    name: String,
    #[serde(flatten)]
    metric: MetricView,
    #[serde(default)]
    inclusions: InclusionsView,
}

impl HeaderView {
    fn from_diff(header: &HeaderDiff) -> Self {
        Self {
            name: header.name().to_string(),
            metric: MetricView::from_diff(&header.parse_duration),
            inclusions: InclusionsView::from_inclusions(
                header.base_inclusions,
                header.target_inclusions,
            ),
        }
    }
}

impl From<HeaderView> for HeaderDiff {
    fn from(view: HeaderView) -> Self {
        Self {
            parse_duration: view.metric.into_diff(view.name),
            base_inclusions: view.inclusions.base,
            target_inclusions: view.inclusions.target,
        }
    }
}

/// Build totals, keyed by phase.
#[derive(Debug, Serialize, Deserialize)]
struct TotalsView {
    total: MetricView,
    frontend: MetricView,
    backend: MetricView,
    other: MetricView,
    template_instantiation: MetricView,
}

impl TotalsView {
    fn from_diff(totals: &TotalsDiff) -> Self {
        Self {
            total: MetricView::from_diff(&totals.total),
            frontend: MetricView::from_diff(&totals.frontend),
            backend: MetricView::from_diff(&totals.backend),
            other: MetricView::from_diff(&totals.other),
            template_instantiation: MetricView::from_diff(&totals.template_instantiation),
        }
    }
}

/// Template rows, separated by template kind.
#[derive(Debug, Default, Serialize, Deserialize)]
struct TemplatesView {
    #[serde(default)]
    functions: Vec<NamedMetricView>,
    #[serde(default)]
    classes: Vec<NamedMetricView>,
}

/// The serialized form of a report comparison.
#[derive(Debug, Serialize, Deserialize)]
struct ReportDiffView {
    schema_version: u32,
    #[serde(default)]
    thresholds: Thresholds,
    totals: TotalsView,
    #[serde(default)]
    targets: Option<Vec<NamedMetricView>>,
    #[serde(default)]
    headers: Vec<HeaderView>,
    #[serde(default)]
    templates: TemplatesView,
    #[serde(default)]
    units: Vec<NamedMetricView>,
}

impl ReportDiffView {
    fn from_diff(diff: &ReportDiff, thresholds: &Thresholds) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            thresholds: *thresholds,
            totals: TotalsView::from_diff(&diff.totals),
            targets: diff
                .targets
                .as_ref()
                .map(|targets| targets.iter().map(NamedMetricView::from_diff).collect()),
            headers: diff.headers.iter().map(HeaderView::from_diff).collect(),
            templates: TemplatesView {
                functions: diff
                    .function_templates
                    .iter()
                    .map(NamedMetricView::from_diff)
                    .collect(),
                classes: diff
                    .class_templates
                    .iter()
                    .map(NamedMetricView::from_diff)
                    .collect(),
            },
            units: diff.units.iter().map(NamedMetricView::from_diff).collect(),
        }
    }

    fn into_document(self) -> ComparisonDocument {
        let thresholds = self.thresholds;
        let totals = self.totals;
        let diff = ReportDiff {
            totals: TotalsDiff {
                total: totals.total.into_diff("total"),
                frontend: totals.frontend.into_diff("frontend"),
                backend: totals.backend.into_diff("backend"),
                other: totals.other.into_diff("other"),
                template_instantiation: totals
                    .template_instantiation
                    .into_diff("template instantiation"),
            },
            targets: self.targets.map(|targets| {
                targets
                    .into_iter()
                    .map(NamedMetricView::into_diff)
                    .collect()
            }),
            headers: self.headers.into_iter().map(HeaderDiff::from).collect(),
            function_templates: self
                .templates
                .functions
                .into_iter()
                .map(NamedMetricView::into_diff)
                .collect(),
            class_templates: self
                .templates
                .classes
                .into_iter()
                .map(NamedMetricView::into_diff)
                .collect(),
            units: self
                .units
                .into_iter()
                .map(NamedMetricView::into_diff)
                .collect(),
        };
        ComparisonDocument { diff, thresholds }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_diff() -> ReportDiff {
        ReportDiff {
            totals: TotalsDiff {
                total: DurationDiff::new("total", Some(1000), Some(1200)),
                frontend: DurationDiff::new("frontend", Some(800), Some(900)),
                backend: DurationDiff::new("backend", Some(100), Some(100)),
                other: DurationDiff::new("other", Some(100), Some(200)),
                template_instantiation: DurationDiff::new(
                    "template instantiation",
                    Some(50),
                    Some(60),
                ),
            },
            targets: Some(vec![DurationDiff::new("lib", Some(400), Some(500))]),
            headers: vec![HeaderDiff {
                parse_duration: DurationDiff::new("include/a.h", Some(100), Some(160)),
                base_inclusions: Some(2),
                target_inclusions: Some(3),
            }],
            function_templates: vec![DurationDiff::new("f<int>", Some(10), Some(20))],
            class_templates: vec![DurationDiff::new("C<int>", Some(30), Some(30))],
            units: vec![DurationDiff::new("src/a.cpp", None, Some(50))],
        }
    }

    fn sample_thresholds() -> Thresholds {
        Thresholds {
            min_delta_us: 10,
            min_percent: 1.0,
            min_inclusions: 1,
        }
    }

    #[test]
    fn round_trips_through_yaml() {
        let diff = sample_diff();
        let thresholds = sample_thresholds();
        let yaml = to_string(&diff, &thresholds).unwrap();

        assert!(yaml.contains("schema_version: 1"));
        assert!(yaml.contains("min_delta_us: 10"));
        assert!(yaml.contains("base_us: 1000"));
        assert!(yaml.contains("change: slower"));
        assert!(yaml.contains("inclusions:"));

        let document = from_str(&yaml).unwrap();
        assert_eq!(document.diff, diff);
        assert_eq!(document.thresholds, thresholds);
    }

    #[test]
    fn rejects_unknown_schema_versions() {
        let yaml = "schema_version: 99\ntotals:\n  total: {base_us: 1, target_us: 2}\n  frontend: {base_us: 1, target_us: 2}\n  backend: {base_us: 1, target_us: 2}\n  other: {base_us: 1, target_us: 2}\n  template_instantiation: {base_us: 1, target_us: 2}\n";
        let err = from_str(yaml).unwrap_err();
        assert!(matches!(
            err,
            YamlError::UnsupportedVersion { found: 99, .. }
        ));
    }

    #[test]
    fn missing_thresholds_use_defaults() {
        let yaml = "schema_version: 1\ntotals:\n  total: {base_us: 1, target_us: 2}\n  frontend: {base_us: 1, target_us: 2}\n  backend: {base_us: 1, target_us: 2}\n  other: {base_us: 1, target_us: 2}\n  template_instantiation: {base_us: 1, target_us: 2}\n";
        let document = from_str(yaml).unwrap();
        assert_eq!(document.thresholds, Thresholds::default());
        assert_eq!(document.diff.totals.total.delta_micros(), Some(1));
    }

    #[test]
    fn recomputes_derived_fields_from_base_and_target() {
        // Unknown fields are ignored on read.
        let yaml = "schema_version: 1\ntotals:\n  total: {base_us: 1, target_us: 2}\n  frontend: {base_us: 1, target_us: 2}\n  backend: {base_us: 1, target_us: 2}\n  other: {base_us: 1, target_us: 2}\n  template_instantiation: {base_us: 1, target_us: 2}\nunits:\n  - name: a.cpp\n    base_us: 100\n    target_us: 90\n    delta_us: 999\n    percent: 999.0\n    change: slower\n    significant: true\n";
        let document = from_str(yaml).unwrap();
        let unit = &document.diff.units[0];
        assert_eq!(unit.change(), Change::Faster);
        assert_eq!(unit.delta_micros(), Some(-10));
        assert_eq!(unit.percent_change(), Some(-10.0));
    }

    #[test]
    fn does_not_store_significance() {
        let mut diff = sample_diff();
        diff.units = vec![DurationDiff::new("src/a.cpp", Some(100_000), Some(100_500))];
        let thresholds = Thresholds {
            min_delta_us: 1000,
            min_percent: 0.1,
            min_inclusions: 1,
        };

        let yaml = to_string(&diff, &thresholds).unwrap();
        assert!(!yaml.contains("significant"));
        assert_eq!(from_str(&yaml).unwrap().thresholds, thresholds);
    }

    #[test]
    fn missing_targets_serializes_as_null_and_reads_back() {
        let mut diff = sample_diff();
        diff.targets = None;
        let yaml = to_string(&diff, &Thresholds::default()).unwrap();
        assert!(yaml.contains("targets: null"));
        assert_eq!(from_str(&yaml).unwrap().diff.targets, None);
    }
}

//! Builds a resolved, serializable summary of a report.
//!
//! The summary is the data the `report` command shows: it replaces the
//! interned string ids with their labels so it can be printed or serialized
//! to JSON.

use std::collections::HashMap;
use std::fmt;

use serde::{Serialize, Serializer};

use crate::analyzer::TemplateInstantiation;
use crate::build_info::{BuildInfo, Tool};
use crate::header_usage::HeaderUsage;
use crate::interner::StringId;
use crate::report::Report;

/// The number of inclusion chains kept for each header in the summary.
const MAX_CHAINS: usize = 5;

/// A duration in seconds.
///
/// It serializes as a number rounded to three decimals, and displays on the
/// terminal with an adaptive unit (s, ms, or us).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Seconds(pub f64);

impl Seconds {
    /// Converts a duration in microseconds.
    fn from_micros(micros: i64) -> Self {
        Seconds(micros as f64 / 1_000_000.0)
    }
}

impl Serialize for Seconds {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_f64((self.0 * 1000.0).round() / 1000.0)
    }
}

impl fmt::Display for Seconds {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let seconds = self.0;
        if seconds >= 1.0 {
            write!(f, "{seconds:.3} s")
        } else if seconds >= 0.001 {
            write!(f, "{:.3} ms", seconds * 1000.0)
        } else {
            write!(f, "{:.0} us", seconds * 1_000_000.0)
        }
    }
}

/// A resolved summary of a report, suitable for terminal or JSON output.
///
/// Every duration is a [`Seconds`]; its field name says so.
#[derive(Debug, Serialize)]
pub struct ReportSummary {
    pub build_info: BuildInfo,
    pub totals: Totals,
    pub longest_units: Vec<UnitDuration>,
    pub longest_headers: Vec<HeaderDuration>,
    pub units: Vec<UnitSummary>,
    pub largest_templates: Vec<TemplateSummary>,
    pub header_usage: Vec<HeaderUsageSummary>,
    pub missing_includes: Vec<MissingIncludeSummary>,
}

/// Build-wide totals, mirroring the overview donut.
#[derive(Debug, Serialize)]
pub struct Totals {
    pub total_seconds: Seconds,
    pub frontend_seconds: Seconds,
    pub backend_seconds: Seconds,
    pub other_seconds: Seconds,
    pub template_instantiation_seconds: Seconds,
    pub unit_count: usize,
    pub header_count: usize,
    pub template_count: usize,
}

/// A translation unit and its total compile duration.
#[derive(Debug, Serialize)]
pub struct UnitDuration {
    pub unit: String,
    pub total_seconds: Seconds,
}

/// A header and its parse statistics.
#[derive(Debug, Serialize)]
pub struct HeaderDuration {
    pub header: String,
    pub parse_seconds: Seconds,
    pub num_inclusions: i64,
    pub avg_inclusion_seconds: Seconds,
    pub inclusion_paths: Vec<InclusionPath>,
}

/// One inclusion chain that reaches a header, and how often.
#[derive(Debug, Serialize)]
pub struct InclusionPath {
    /// Full path from an includer to the header, ending in the header itself.
    pub chain: Vec<String>,
    pub count: i64,
}

/// Everything the summary knows about one translation unit.
#[derive(Debug, Serialize)]
pub struct UnitSummary {
    pub unit: String,
    pub total_seconds: Seconds,
    pub frontend_seconds: Seconds,
    pub backend_seconds: Seconds,
    pub other_seconds: Seconds,
    pub template_instantiation_seconds: Seconds,
    pub largest_function: Option<TemplateSummary>,
    pub largest_class: Option<TemplateSummary>,
    pub header_parse: Vec<HeaderTime>,
    pub header_usage: Vec<HeaderUsageEntry>,
    pub missing_includes: Vec<String>,
}

/// A header's parse time within one translation unit.
#[derive(Debug, Serialize)]
pub struct HeaderTime {
    pub header: String,
    pub seconds: Seconds,
}

/// How one translation unit uses an included header.
#[derive(Debug, Serialize)]
pub struct HeaderUsageEntry {
    pub header: String,
    /// `Unused`, `DeclarationOnly`, or `NeedsDefinition`.
    pub usage: String,
}

/// The duration and count of one template instantiation.
#[derive(Debug, Serialize)]
pub struct TemplateSummary {
    /// `function` or `class`; absent for the per-unit summaries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    pub name: String,
    pub total_seconds: Seconds,
    pub num_instantiations: i64,
    pub avg_seconds: Seconds,
}

/// A header that one or more units should include directly.
#[derive(Debug, Serialize)]
pub struct MissingIncludeSummary {
    pub header: String,
    pub unit_count: usize,
}

/// How a header is used across the build.
#[derive(Debug, Serialize)]
pub struct HeaderUsageSummary {
    pub header: String,
    pub unused_units: usize,
    pub forward_declarable_units: usize,
    pub definition_required_units: usize,
}

/// Converts a duration in microseconds to seconds.
fn to_seconds(micros: i64) -> Seconds {
    Seconds::from_micros(micros)
}

/// Sorts usage entries by severity, then by name.
fn usage_rank(usage: &str) -> u8 {
    match usage {
        "Unused" => 0,
        "DeclarationOnly" => 1,
        "NeedsDefinition" => 2,
        _ => 3,
    }
}

// ---------------------------------------------------------------------------
// Terminal formatting helpers
// ---------------------------------------------------------------------------

/// Prints a section title with a rule underneath.
fn print_section(title: &str) {
    println!("{title}");
    println!("{}", "─".repeat(title.chars().count()));
}

/// Formats a duration as a percentage of `total`.
fn percent(value: Seconds, total: Seconds) -> String {
    if total.0 <= 0.0 {
        "0.0%".to_string()
    } else {
        format!("{:.1}%", value.0 / total.0 * 100.0)
    }
}

/// Formats a count with its noun in the right number.
fn plural(count: i64, singular: &str, plural: &str) -> String {
    format!("{count} {}", if count == 1 { singular } else { plural })
}

/// Formats a ranked row with the metric first: `duration (percent)  subject`.
fn format_ranked_row(metric: Seconds, total: Seconds, subject: &str) -> String {
    format!("{metric} ({})  {subject}", percent(metric, total))
}

/// Formats one inclusion chain, naming direct inclusions explicitly instead of
/// repeating the header's own path.
fn format_inclusion_path(path: &InclusionPath) -> String {
    if path.chain.len() <= 1 {
        "direct inclusion".to_string()
    } else {
        format!("via {}", path.chain.join(" -> "))
    }
}

/// Formats one tool as `name version`.
fn format_tool(tool: &Tool) -> String {
    if tool.version.is_empty() {
        tool.name.clone()
    } else {
        format!("{} {}", tool.name, tool.version)
    }
}

/// Builds a one-line build configuration header.
fn build_header(build_info: &BuildInfo) -> String {
    let mut parts = Vec::new();
    if let Some(toolchain) = &build_info.toolchain {
        for tool in [
            &toolchain.build_system,
            &toolchain.generator,
            &toolchain.cxx_compiler,
            &toolchain.c_compiler,
            &toolchain.linker,
        ]
        .into_iter()
        .flatten()
        {
            parts.push(format_tool(tool));
        }
    }
    if let Some(commit) = &build_info.git_commit {
        parts.push(format!("commit {commit}"));
    }
    if parts.is_empty() {
        "Build: (no build info)".to_string()
    } else {
        format!("Build: {}", parts.join(", "))
    }
}

impl ReportSummary {
    /// Builds the summary from a report. `limit` caps the ranked top-N lists.
    pub fn from_report(report: &Report, limit: usize) -> Self {
        let analysis = &report.analysis;
        let per_unit_template =
            |name_id: &StringId, inst: &TemplateInstantiation| TemplateSummary {
                kind: None,
                name: report.resolve(*name_id).to_string(),
                total_seconds: to_seconds(inst.total_duration),
                num_instantiations: inst.num_instantiations,
                avg_seconds: to_seconds(inst.avg_duration()),
            };

        let mut total_duration = 0i64;
        let mut frontend_duration = 0i64;
        let mut backend_duration = 0i64;
        let mut template_instantiation_duration = 0i64;

        let mut units: Vec<UnitSummary> = analysis
            .unit_analysis
            .iter()
            .map(|(unit_id, unit)| {
                let other =
                    (unit.total_duration - unit.frontend_duration - unit.backend_duration).max(0);
                total_duration += unit.total_duration;
                frontend_duration += unit.frontend_duration;
                backend_duration += unit.backend_duration;
                template_instantiation_duration += unit.template_instantiation_duration;

                let mut header_parse: Vec<HeaderTime> = unit
                    .header_parse_duration
                    .iter()
                    .map(|(header, duration)| HeaderTime {
                        header: report.resolve(*header).to_string(),
                        seconds: to_seconds(*duration),
                    })
                    .collect();
                header_parse.sort_by(|a, b| {
                    b.seconds
                        .0
                        .total_cmp(&a.seconds.0)
                        .then_with(|| a.header.cmp(&b.header))
                });

                let mut header_usage: Vec<HeaderUsageEntry> = unit
                    .header_usage
                    .iter()
                    .map(|(header, usage)| HeaderUsageEntry {
                        header: report.resolve(*header).to_string(),
                        usage: match usage {
                            HeaderUsage::Unused => "Unused",
                            HeaderUsage::DeclarationOnly => "DeclarationOnly",
                            HeaderUsage::NeedsDefinition => "NeedsDefinition",
                        }
                        .to_string(),
                    })
                    .collect();
                header_usage.sort_by(|a, b| {
                    usage_rank(&a.usage)
                        .cmp(&usage_rank(&b.usage))
                        .then_with(|| a.header.cmp(&b.header))
                });

                UnitSummary {
                    unit: report.resolve(*unit_id).to_string(),
                    total_seconds: to_seconds(unit.total_duration),
                    frontend_seconds: to_seconds(unit.frontend_duration),
                    backend_seconds: to_seconds(unit.backend_duration),
                    other_seconds: to_seconds(other),
                    template_instantiation_seconds: to_seconds(
                        unit.template_instantiation_duration,
                    ),
                    largest_function: unit
                        .largest_function_template_instantiation()
                        .map(|(id, inst)| per_unit_template(id, inst)),
                    largest_class: unit
                        .largest_class_template_instantiation()
                        .map(|(id, inst)| per_unit_template(id, inst)),
                    header_parse,
                    header_usage,
                    missing_includes: {
                        let mut missing: Vec<String> = unit
                            .missing_includes
                            .iter()
                            .map(|id| report.resolve(*id).to_string())
                            .collect();
                        missing.sort();
                        missing.dedup();
                        missing
                    },
                }
            })
            .collect();
        units.sort_by(|a, b| a.unit.cmp(&b.unit));

        let mut missing_counts: HashMap<String, usize> = HashMap::new();
        for unit in &units {
            for header in &unit.missing_includes {
                *missing_counts.entry(header.clone()).or_default() += 1;
            }
        }
        let mut missing_includes: Vec<MissingIncludeSummary> = missing_counts
            .into_iter()
            .map(|(header, unit_count)| MissingIncludeSummary { header, unit_count })
            .collect();
        missing_includes.sort_by(|a, b| {
            b.unit_count
                .cmp(&a.unit_count)
                .then_with(|| a.header.cmp(&b.header))
        });

        let totals = Totals {
            total_seconds: to_seconds(total_duration),
            frontend_seconds: to_seconds(frontend_duration),
            backend_seconds: to_seconds(backend_duration),
            other_seconds: to_seconds(
                (total_duration - frontend_duration - backend_duration).max(0),
            ),
            template_instantiation_seconds: to_seconds(template_instantiation_duration),
            unit_count: analysis.unit_analysis.len(),
            header_count: analysis.header_analysis.len(),
            template_count: analysis.function_template_instantiations.len()
                + analysis.class_template_instantiations.len(),
        };

        let longest_units = analysis
            .longest_unit_compilations(limit)
            .map(|(id, unit)| UnitDuration {
                unit: report.resolve(*id).to_string(),
                total_seconds: to_seconds(unit.total_duration),
            })
            .collect();

        let longest_headers = analysis
            .longest_headers_parse_duration(limit)
            .map(|(id, header)| {
                let name = report.resolve(*id);
                let mut chains: Vec<(&Vec<StringId>, &i64)> =
                    header.inclusion_paths.iter().collect();
                chains.sort_by(|a, b| b.1.cmp(a.1));
                let inclusion_paths = chains
                    .into_iter()
                    .take(MAX_CHAINS)
                    .map(|(chain, count)| {
                        let mut resolved: Vec<String> = chain
                            .iter()
                            .map(|c| report.resolve(*c).to_string())
                            .collect();
                        resolved.push(name.to_string());
                        InclusionPath {
                            chain: resolved,
                            count: *count,
                        }
                    })
                    .collect();
                HeaderDuration {
                    header: name.to_string(),
                    parse_seconds: to_seconds(header.parse_duration),
                    num_inclusions: header.num_inclusions,
                    avg_inclusion_seconds: to_seconds(header.avg_inclusion_time()),
                    inclusion_paths,
                }
            })
            .collect();

        let largest_templates = analysis
            .longest_template_instantiations(limit)
            .map(|(kind, name_id, inst)| TemplateSummary {
                kind: Some(kind.to_string()),
                name: report.resolve(*name_id).to_string(),
                total_seconds: to_seconds(inst.total_duration),
                num_instantiations: inst.num_instantiations,
                avg_seconds: to_seconds(inst.avg_duration()),
            })
            .collect();

        let mut header_usage: Vec<HeaderUsageSummary> = analysis
            .header_analysis
            .iter()
            .filter(|(_, header)| {
                header.unused_units
                    + header.forward_declarable_units
                    + header.definition_required_units
                    > 0
            })
            .map(|(id, header)| HeaderUsageSummary {
                header: report.resolve(*id).to_string(),
                unused_units: header.unused_units,
                forward_declarable_units: header.forward_declarable_units,
                definition_required_units: header.definition_required_units,
            })
            .collect();
        header_usage.sort_by(|a, b| {
            let rank =
                |h: &HeaderUsageSummary| h.definition_required_units + h.forward_declarable_units;
            rank(b).cmp(&rank(a)).then_with(|| a.header.cmp(&b.header))
        });

        Self {
            build_info: report.build_info.clone(),
            totals,
            longest_units,
            longest_headers,
            units,
            largest_templates,
            header_usage,
            missing_includes,
        }
    }

    /// Prints the summary to the terminal. `limit` caps the per-unit section
    /// as well as the ranked lists.
    pub fn print(&self, limit: usize) {
        println!("{}", build_header(&self.build_info));

        println!();
        print_section("Totals");
        println!("  total: {}", self.totals.total_seconds);
        println!("  frontend: {}", self.totals.frontend_seconds);
        println!("  backend: {}", self.totals.backend_seconds);
        println!("  other: {}", self.totals.other_seconds);
        println!(
            "  template instantiation: {}",
            self.totals.template_instantiation_seconds
        );
        println!("  translation units: {}", self.totals.unit_count);
        println!("  headers: {}", self.totals.header_count);

        self.print_longest_units();

        self.print_longest_headers();

        self.print_largest_templates();

        self.print_header_usage();

        self.print_missing_includes();

        self.print_per_unit(limit);
    }

    fn print_longest_units(&self) {
        println!();
        print_section(&format!(
            "Longest files to compile ({} of {})",
            self.longest_units.len(),
            self.totals.unit_count
        ));
        for unit in &self.longest_units {
            println!(
                "{}",
                format_ranked_row(unit.total_seconds, self.totals.total_seconds, &unit.unit)
            );
        }
    }

    fn print_longest_headers(&self) {
        println!();
        print_section(&format!(
            "Longest headers to parse ({} of {})",
            self.longest_headers.len(),
            self.totals.header_count
        ));
        for header in &self.longest_headers {
            println!(
                "{} ({})  {} ({}, avg {})",
                header.parse_seconds,
                percent(header.parse_seconds, self.totals.total_seconds),
                header.header,
                plural(header.num_inclusions, "inclusion", "inclusions"),
                header.avg_inclusion_seconds
            );
            for path in &header.inclusion_paths {
                println!("   {}× {}", path.count, format_inclusion_path(path));
            }
        }
    }

    fn print_largest_templates(&self) {
        println!();
        print_section(&format!(
            "Largest template instantiations in the entire build ({} of {})",
            self.largest_templates.len(),
            self.totals.template_count
        ));
        for template in &self.largest_templates {
            println!(
                "{} ({})  {} template '{}' ({}, avg {})",
                template.total_seconds,
                percent(template.total_seconds, self.totals.total_seconds),
                template.kind.as_deref().unwrap_or(""),
                template.name,
                plural(
                    template.num_instantiations,
                    "instantiation",
                    "instantiations"
                ),
                template.avg_seconds
            );
        }
    }

    fn print_header_usage(&self) {
        println!();
        print_section(&format!("Header usage ({})", self.header_usage.len()));
        if self.header_usage.is_empty() {
            println!("  (no header-usage data in this report)");
            return;
        }
        for header in &self.header_usage {
            if header.unused_units > 0 {
                println!(
                    "  {} unused: {} -> remove #include",
                    plural(header.unused_units as i64, "unit", "units"),
                    header.header
                );
            }
            if header.forward_declarable_units > 0 {
                println!(
                    "  {} forward-declarable: {} -> replace #include with forward declaration",
                    plural(header.forward_declarable_units as i64, "unit", "units"),
                    header.header
                );
            }
            if header.definition_required_units > 0 {
                println!(
                    "  {} with definition required: {}",
                    plural(header.definition_required_units as i64, "unit", "units"),
                    header.header
                );
            }
        }
    }

    fn print_missing_includes(&self) {
        println!();
        print_section(&format!(
            "Missing direct includes ({})",
            self.missing_includes.len()
        ));
        if self.missing_includes.is_empty() {
            println!("  (none)");
            return;
        }
        for header in &self.missing_includes {
            println!(
                "  {}: {} -> add direct #include",
                plural(header.unit_count as i64, "unit", "units"),
                header.header
            );
        }
    }

    fn print_per_unit(&self, limit: usize) {
        println!();
        let shown = limit.min(self.units.len());
        print_section(&format!(
            "Per translation unit ({shown} of {})",
            self.units.len()
        ));

        let mut ordered: Vec<&UnitSummary> = self.units.iter().collect();
        ordered.sort_by(|a, b| {
            b.total_seconds
                .0
                .total_cmp(&a.total_seconds.0)
                .then_with(|| a.unit.cmp(&b.unit))
        });

        for (index, unit) in ordered.into_iter().take(limit).enumerate() {
            if index > 0 {
                println!();
            }
            println!(
                "{}",
                format_ranked_row(unit.total_seconds, self.totals.total_seconds, &unit.unit)
            );
            println!(
                "  frontend {} | backend {} | other {} | template instantiation {}",
                unit.frontend_seconds,
                unit.backend_seconds,
                unit.other_seconds,
                unit.template_instantiation_seconds
            );
            println!(
                "  {}",
                format_template_line("function", unit.largest_function.as_ref())
            );
            println!(
                "  {}",
                format_template_line("class", unit.largest_class.as_ref())
            );
            if !unit.header_parse.is_empty() {
                let shown_headers = limit.min(unit.header_parse.len());
                println!(
                    "  headers ({shown_headers} of {}):",
                    unit.header_parse.len()
                );
                for header in &unit.header_parse[..shown_headers] {
                    println!(
                        "    {}",
                        format_ranked_row(header.seconds, unit.total_seconds, &header.header)
                    );
                }
            }
            if !unit.header_usage.is_empty() {
                println!("  usage:");
                for header in &unit.header_usage {
                    println!("    {}: {}", header.header, header.usage);
                }
            }
            if !unit.missing_includes.is_empty() {
                println!(
                    "  add direct includes: {}",
                    unit.missing_includes.join(", ")
                );
            }
        }
    }
}

/// Formats a per-unit template summary with the duration first.
fn format_template_line(kind: &str, template: Option<&TemplateSummary>) -> String {
    match template {
        Some(t) => format!(
            "{}  {kind} template '{}' ({}, avg {})",
            t.total_seconds,
            t.name,
            plural(t.num_instantiations, "instantiation", "instantiations"),
            t.avg_seconds
        ),
        None => format!("{kind} template: none"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::{AnalysisResult, UnitAnalysis};
    use std::collections::{BTreeMap, HashMap};

    fn id(strings: &[String], s: &str) -> StringId {
        StringId::from(strings.binary_search_by(|x| x.as_str().cmp(s)).unwrap() as u32)
    }

    fn report_with_one_unit() -> Report {
        let strings = vec!["a.cpp".to_string(), "my_template".to_string()];
        let mut unit = UnitAnalysis {
            total_duration: 1_234_567,
            frontend_duration: 1_000_000,
            backend_duration: 200_000,
            template_instantiation_duration: 100_000,
            header_parse_duration: HashMap::new(),
            header_inclusion_paths: BTreeMap::new(),
            header_usage: HashMap::new(),
            function_template_instantiations: HashMap::new(),
            class_template_instantiations: HashMap::new(),
            missing_includes: Vec::new(),
        };
        unit.function_template_instantiations.insert(
            id(&strings, "my_template"),
            TemplateInstantiation::new(1_234_567),
        );
        let units = HashMap::from([(id(&strings, "a.cpp"), unit)]);
        Report::new(strings, BuildInfo::default(), AnalysisResult::from(units))
    }

    #[test]
    fn resolves_names_and_converts_durations_to_seconds() {
        let report = report_with_one_unit();
        let summary = ReportSummary::from_report(&report, 10);

        assert_eq!(summary.totals.total_seconds, Seconds(1.234567));
        assert_eq!(summary.totals.total_seconds.to_string(), "1.235 s");
        assert_eq!(summary.totals.frontend_seconds, Seconds(1.0));
        assert_eq!(summary.totals.backend_seconds, Seconds(0.2));
        assert_eq!(summary.totals.other_seconds, Seconds(0.034567));
        assert_eq!(summary.totals.other_seconds.to_string(), "34.567 ms");
        assert_eq!(summary.totals.template_instantiation_seconds, Seconds(0.1));
        assert_eq!(summary.totals.unit_count, 1);

        assert_eq!(summary.longest_units[0].unit, "a.cpp");
        assert_eq!(summary.longest_units[0].total_seconds, Seconds(1.234567));

        let unit = &summary.units[0];
        assert_eq!(unit.unit, "a.cpp");
        assert_eq!(unit.other_seconds, Seconds(0.034567));
        let t = unit.largest_function.as_ref().unwrap();
        assert_eq!(t.name, "my_template");
        assert_eq!(t.avg_seconds, Seconds(1.234567));
        assert!(t.kind.is_none());

        assert_eq!(summary.largest_templates[0].name, "my_template");
        assert_eq!(
            summary.largest_templates[0].kind.as_deref(),
            Some("function")
        );
    }

    #[test]
    fn json_rounds_seconds_to_three_decimals() {
        let report = report_with_one_unit();
        let summary = ReportSummary::from_report(&report, 10);
        let json = serde_json::to_value(&summary).unwrap();
        assert_eq!(json["totals"]["total_seconds"], 1.235);
        assert_eq!(json["totals"]["other_seconds"], 0.035);
    }

    #[test]
    fn result_rows_put_the_metric_first() {
        assert_eq!(plural(1, "inclusion", "inclusions"), "1 inclusion");
        assert_eq!(plural(2, "inclusion", "inclusions"), "2 inclusions");
        assert_eq!(
            format_ranked_row(Seconds(0.214749), Seconds(1.0), "src/main.cpp"),
            "214.749 ms (21.5%)  src/main.cpp"
        );

        let direct = InclusionPath {
            chain: vec!["a.h".to_string()],
            count: 7,
        };
        assert_eq!(format_inclusion_path(&direct), "direct inclusion");
        let indirect = InclusionPath {
            chain: vec!["a.h".to_string(), "b.h".to_string()],
            count: 1,
        };
        assert_eq!(format_inclusion_path(&indirect), "via a.h -> b.h");

        let template = TemplateSummary {
            kind: None,
            name: "foo".to_string(),
            total_seconds: Seconds(0.00126),
            num_instantiations: 1,
            avg_seconds: Seconds(0.00126),
        };
        assert_eq!(
            format_template_line("function", Some(&template)),
            "1.260 ms  function template 'foo' (1 instantiation, avg 1.260 ms)"
        );
        assert_eq!(format_template_line("class", None), "class template: none");
    }

    #[test]
    fn sub_second_durations_switch_units() {
        assert_eq!(Seconds(0.5).to_string(), "500.000 ms");
        assert_eq!(Seconds(0.0005).to_string(), "500 us");
        assert_eq!(Seconds(2.0).to_string(), "2.000 s");
    }
}

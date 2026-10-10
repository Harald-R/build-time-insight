//! Builds a resolved, serializable summary of a report.
//!
//! The summary is the data the `report` command shows: it replaces the
//! interned string ids with their labels so it can be printed or serialized
//! to JSON.

use serde::Serialize;

use crate::analyzer::TemplateInstantiation;
use crate::build_info::BuildInfo;
use crate::interner::StringId;
use crate::report::Report;

/// The number of inclusion chains kept for each header in the summary.
const MAX_CHAINS: usize = 5;

/// A resolved summary of a report, suitable for terminal or JSON output.
#[derive(Debug, Serialize)]
pub struct ReportSummary {
    pub build_info: BuildInfo,
    pub longest_units: Vec<UnitDuration>,
    pub longest_headers: Vec<HeaderDuration>,
    pub templates_per_unit: Vec<UnitTemplates>,
    pub largest_templates: Vec<TemplateSummary>,
    pub header_usage: Vec<HeaderUsageSummary>,
}

/// A translation unit and its total compile duration.
#[derive(Debug, Serialize)]
pub struct UnitDuration {
    pub unit: String,
    pub total_duration: i64,
}

/// A header and its parse statistics.
#[derive(Debug, Serialize)]
pub struct HeaderDuration {
    pub header: String,
    pub parse_duration: i64,
    pub num_inclusions: i64,
    pub avg_inclusion_time: i64,
    pub inclusion_paths: Vec<InclusionPath>,
}

/// One inclusion chain that reaches a header, and how often.
#[derive(Debug, Serialize)]
pub struct InclusionPath {
    /// Full path from an includer to the header, ending in the header itself.
    pub chain: Vec<String>,
    pub count: i64,
}

/// The largest template instantiations for one translation unit.
#[derive(Debug, Serialize)]
pub struct UnitTemplates {
    pub unit: String,
    pub function: Option<TemplateSummary>,
    pub class: Option<TemplateSummary>,
}

/// The duration and count of one template instantiation.
#[derive(Debug, Serialize)]
pub struct TemplateSummary {
    /// `function` or `class`; absent for the per-unit summaries.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    pub name: String,
    pub total_duration: i64,
    pub num_instantiations: i64,
    pub avg_duration: i64,
}

/// How a header is used across the build.
#[derive(Debug, Serialize)]
pub struct HeaderUsageSummary {
    pub header: String,
    pub unused_units: usize,
    pub forward_declarable_units: usize,
    pub definition_required_units: usize,
}

impl ReportSummary {
    /// Builds the summary from a report. `limit` caps each top-N list.
    pub fn from_report(report: &Report, limit: usize) -> Self {
        let analysis = &report.analysis;
        let per_unit_template =
            |name_id: &StringId, inst: &TemplateInstantiation| TemplateSummary {
                kind: None,
                name: report.resolve(*name_id).to_string(),
                total_duration: inst.total_duration,
                num_instantiations: inst.num_instantiations,
                avg_duration: inst.avg_duration(),
            };

        let longest_units = analysis
            .longest_unit_compilations(limit)
            .map(|(id, unit)| UnitDuration {
                unit: report.resolve(*id).to_string(),
                total_duration: unit.total_duration,
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
                    parse_duration: header.parse_duration,
                    num_inclusions: header.num_inclusions,
                    avg_inclusion_time: header.avg_inclusion_time(),
                    inclusion_paths,
                }
            })
            .collect();

        let mut templates_per_unit: Vec<UnitTemplates> = analysis
            .unit_analysis
            .iter()
            .map(|(unit_id, unit)| UnitTemplates {
                unit: report.resolve(*unit_id).to_string(),
                function: unit
                    .largest_function_template_instantiation()
                    .map(|(id, inst)| per_unit_template(id, inst)),
                class: unit
                    .largest_class_template_instantiation()
                    .map(|(id, inst)| per_unit_template(id, inst)),
            })
            .collect();
        templates_per_unit.sort_by(|a, b| a.unit.cmp(&b.unit));

        let largest_templates = analysis
            .longest_template_instantiations(limit)
            .map(|(kind, name_id, inst)| TemplateSummary {
                kind: Some(kind.to_string()),
                name: report.resolve(*name_id).to_string(),
                total_duration: inst.total_duration,
                num_instantiations: inst.num_instantiations,
                avg_duration: inst.avg_duration(),
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
            longest_units,
            longest_headers,
            templates_per_unit,
            largest_templates,
            header_usage,
        }
    }

    /// Prints the summary to the terminal.
    pub fn print(&self) {
        println!("Longest files to compile:");
        for (rank, unit) in self.longest_units.iter().enumerate() {
            println!("{}. {}: {} us", rank + 1, unit.unit, unit.total_duration);
        }

        println!("Longest headers to parse:");
        for (rank, header) in self.longest_headers.iter().enumerate() {
            println!(
                "{}. {}: {} us (included {} times, avg {} us/inclusion)",
                rank + 1,
                header.header,
                header.parse_duration,
                header.num_inclusions,
                header.avg_inclusion_time
            );
            for path in &header.inclusion_paths {
                println!("   via {} ({} times)", path.chain.join(" -> "), path.count);
            }
        }

        println!("Largest template instantiations per translation unit:");
        for unit in &self.templates_per_unit {
            println!(
                "{}: function: {}, class: {}",
                unit.unit,
                format_template(unit.function.as_ref()),
                format_template(unit.class.as_ref())
            );
        }

        println!("Largest template instantiations in the entire build:");
        for (rank, template) in self.largest_templates.iter().enumerate() {
            println!(
                "{}. {} template '{}': {} us ({} instantiations, avg {} us)",
                rank + 1,
                template.kind.as_deref().unwrap_or(""),
                template.name,
                template.total_duration,
                template.num_instantiations,
                template.avg_duration
            );
        }

        println!("Header usage:");
        if self.header_usage.is_empty() {
            println!("  (no header-usage data in this report)");
        }
        for header in &self.header_usage {
            if header.unused_units > 0 {
                println!(
                    "  {}: unused in {} unit(s) -> remove #include",
                    header.header, header.unused_units
                );
            }
            if header.forward_declarable_units > 0 {
                println!(
                    "  {}: replace #include with forward declaration in {} unit(s)",
                    header.header, header.forward_declarable_units
                );
            }
            if header.definition_required_units > 0 {
                println!(
                    "  {}: needs definition in {} unit(s)",
                    header.header, header.definition_required_units
                );
            }
        }
    }
}

/// Formats a per-unit template summary for the terminal.
fn format_template(template: Option<&TemplateSummary>) -> String {
    match template {
        Some(t) => format!(
            "{} ({} us, {} instantiations, avg {} us)",
            t.name, t.total_duration, t.num_instantiations, t.avg_duration
        ),
        None => "none".to_string(),
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
            total_duration: 100,
            frontend_duration: 60,
            backend_duration: 40,
            template_instantiation_duration: 20,
            header_parse_duration: HashMap::new(),
            header_inclusion_paths: BTreeMap::new(),
            header_usage: HashMap::new(),
            function_template_instantiations: HashMap::new(),
            class_template_instantiations: HashMap::new(),
        };
        unit.function_template_instantiations
            .insert(id(&strings, "my_template"), TemplateInstantiation::new(7));
        let units = HashMap::from([(id(&strings, "a.cpp"), unit)]);
        Report::new(strings, BuildInfo::default(), AnalysisResult::from(units))
    }

    #[test]
    fn resolves_names_and_template_average() {
        let report = report_with_one_unit();
        let summary = ReportSummary::from_report(&report, 10);

        assert_eq!(summary.longest_units.len(), 1);
        assert_eq!(summary.longest_units[0].unit, "a.cpp");
        assert_eq!(summary.longest_units[0].total_duration, 100);

        let t = summary.templates_per_unit[0].function.as_ref().unwrap();
        assert_eq!(t.name, "my_template");
        assert_eq!(t.avg_duration, 7);
        assert!(t.kind.is_none());

        assert_eq!(summary.largest_templates[0].name, "my_template");
        assert_eq!(
            summary.largest_templates[0].kind.as_deref(),
            Some("function")
        );
    }
}

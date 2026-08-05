use std::cmp::min;
use std::collections::HashMap;

use itertools::Itertools;
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};

use crate::build_traces::FileTrace;
use crate::raw_trace::*;

#[derive(Debug)]
pub struct UnitAnalysis {
    pub total_duration: i64,
    pub frontend_duration: i64,
    pub backend_duration: i64,
    pub header_parse_duration: HashMap<String, i64>,
}

#[derive(Debug)]
pub struct HeaderAnalysis {
    pub parse_duration: i64,
    pub num_inclusions: i64,
    // TODO: Inclusion paths
}

#[derive(Debug)]
pub struct AnalysisResult {
    pub unit_analysis: HashMap<String, UnitAnalysis>,
    pub header_analysis: HashMap<String, HeaderAnalysis>,
}

impl AnalysisResult {
    pub fn from(unit_analysis: HashMap<String, UnitAnalysis>) -> Self {
        let header_analysis = Self::get_per_header_analysis(&unit_analysis);
        AnalysisResult {
            unit_analysis,
            header_analysis,
        }
    }

    fn get_per_header_analysis(
        per_unit_analysis: &HashMap<String, UnitAnalysis>,
    ) -> HashMap<String, HeaderAnalysis> {
        let mut header_analysis: HashMap<String, HeaderAnalysis> = HashMap::new();
        for analysis in per_unit_analysis.iter() {
            for (header, duration) in analysis.1.header_parse_duration.iter() {
                header_analysis
                    .entry(header.to_string())
                    .and_modify(|val| {
                        val.parse_duration += duration;
                        val.num_inclusions += 1
                    })
                    .or_insert(HeaderAnalysis {
                        parse_duration: *duration,
                        num_inclusions: 1,
                    });
            }
        }
        header_analysis
    }

    pub fn longest_unit_compilations(
        &self,
        num_items: usize,
    ) -> impl Iterator<Item = (&String, &UnitAnalysis)> {
        let num_items = min(self.unit_analysis.len(), num_items);
        self.unit_analysis.iter().k_largest_by(num_items, |a, b| {
            a.1.total_duration.cmp(&b.1.total_duration)
        })
    }

    pub fn longest_headers_parse_duration(
        &self,
        num_items: usize,
    ) -> impl Iterator<Item = (&String, &HeaderAnalysis)> {
        let num_items = min(self.header_analysis.len(), num_items);
        self.header_analysis.iter().k_largest_by(num_items, |a, b| {
            a.1.parse_duration.cmp(&b.1.parse_duration)
        })
    }
}

pub fn analyze(traces: Vec<FileTrace>) -> AnalysisResult {
    let unit_analysis: HashMap<String, UnitAnalysis> = traces
        .par_iter()
        .map(|trace| {
            let source_file = trace
                .source_file
                .as_deref()
                .unwrap_or(&trace.trace_file)
                .to_string();
            let mut analysis = UnitAnalysis {
                total_duration: 0,
                frontend_duration: 0,
                backend_duration: 0,
                header_parse_duration: HashMap::new(),
            };
            trace.trace_tree.nodes.iter().for_each(|node| {
                if node.trace.dur.is_none() {
                    return;
                }
                let duration = node.trace.dur.unwrap_or(0);
                let name = &node.trace.name;
                if name == EXECUTE_COMPILER {
                    analysis.total_duration += duration;
                } else if name == FRONTEND {
                    analysis.frontend_duration += duration;
                } else if name == BACKEND {
                    analysis.backend_duration += duration;
                } else if name == PARSE_DECL_OR_FN_DEF {
                    if let Some(args) = &node.trace.args {
                        if let Some(detail) = args.get("detail") {
                            if let Some(header) = detail.as_str() {
                                let header = header.splitn(2, ':').next().unwrap_or(header);
                                if header.ends_with(".h") || header.ends_with(".hpp") {
                                    analysis
                                        .header_parse_duration
                                        .entry(header.to_string())
                                        .and_modify(|val| *val += duration)
                                        .or_insert(duration);
                                }
                            }
                        }
                    }
                }
            });
            (source_file, analysis)
        })
        .collect();
    AnalysisResult::from(unit_analysis)
}

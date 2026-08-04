use std::cmp::min;
use std::collections::HashMap;

use itertools::Itertools;
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};

use crate::build_traces::FileTrace;
use crate::raw_trace::*;

#[derive(Debug)]
pub struct Analysis {
    source_file: String,
    total_duration: i64,
    frontend_duration: i64,
    backend_duration: i64,
    header_parse_duration: HashMap<String, i64>,
}

pub fn analyze(traces: Vec<FileTrace>) {
    let per_file_analysis: Vec<Analysis> = traces
        .par_iter()
        .map(|trace| {
            let mut analysis = Analysis {
                source_file: trace
                    .source_file
                    .as_deref()
                    .unwrap_or(&trace.trace_file)
                    .to_string(),
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
            analysis
        })
        .collect();

    println!("{:#?}", per_file_analysis);

    let num_items = min(per_file_analysis.len(), 10);
    let top_k: Vec<&Analysis> = per_file_analysis
        .iter()
        .k_largest_by(num_items, |a, b| a.total_duration.cmp(&b.total_duration))
        .collect();
    println!("Longest files to compile:");
    for analysis in top_k.iter().enumerate() {
        println!(
            "{}. {}: {} us",
            analysis.0 + 1,
            analysis.1.source_file,
            analysis.1.total_duration
        );
    }

    let mut header_total_duration: HashMap<String, i64> = HashMap::new();

    println!("Per-file results:");
    for analysis in per_file_analysis.iter() {
        let sum_header_parse: i64 = analysis
            .header_parse_duration
            .iter()
            .map(|item| {
                let header = item.0;
                let duration = item.1;
                header_total_duration
                    .entry(header.to_string())
                    .and_modify(|val| *val += duration)
                    .or_insert(*duration);
                duration
            })
            .sum();
        println!(
            "{}: {} frontend, {} backend, {} total header parse",
            analysis.source_file,
            analysis.frontend_duration,
            analysis.backend_duration,
            sum_header_parse
        );
    }

    println!("Longest headers to parse:");
    let num_items = min(header_total_duration.len(), 10);
    let top_k: Vec<(&String, &i64)> = header_total_duration
        .iter()
        .k_largest_by(num_items, |a, b| a.1.cmp(&b.1))
        .collect();
    for header in top_k.iter().enumerate() {
        println!("{}. {}: {} us", header.0 + 1, header.1.0, header.1.1);
    }
}

use std::cmp::min;
use std::collections::HashMap;

use itertools::Itertools;
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};

use crate::build_traces::{FileTrace, LoopControl, TraceNode};
use crate::raw_trace::*;

#[derive(Debug)]
pub struct UnitAnalysis {
    pub total_duration: i64,
    pub frontend_duration: i64,
    pub backend_duration: i64,
    pub template_instantiation_duration: i64,
    pub header_parse_duration: HashMap<String, i64>,
    pub header_inclusion_paths: HashMap<String, HashMap<String, i64>>,
    pub function_template_instantiations: HashMap<String, TemplateInstantiation>,
    pub class_template_instantiations: HashMap<String, TemplateInstantiation>,
}

#[derive(Debug, Clone)]
pub struct TemplateInstantiation {
    pub total_duration: i64,
    pub num_instantiations: i64,
}

impl TemplateInstantiation {
    pub fn new(duration: i64) -> Self {
        Self {
            total_duration: duration,
            num_instantiations: 1,
        }
    }

    pub fn add(&mut self, duration: i64) {
        self.total_duration += duration;
        self.num_instantiations += 1;
    }

    pub fn merge(&mut self, other: &Self) {
        self.total_duration += other.total_duration;
        self.num_instantiations += other.num_instantiations;
    }
}

impl UnitAnalysis {
    pub fn largest_function_template_instantiation(
        &self,
    ) -> Option<(&String, &TemplateInstantiation)> {
        self.function_template_instantiations
            .iter()
            .max_by(|a, b| a.1.total_duration.cmp(&b.1.total_duration))
    }

    pub fn largest_class_template_instantiation(
        &self,
    ) -> Option<(&String, &TemplateInstantiation)> {
        self.class_template_instantiations
            .iter()
            .max_by(|a, b| a.1.total_duration.cmp(&b.1.total_duration))
    }
}

#[derive(Debug)]
pub struct HeaderAnalysis {
    pub parse_duration: i64,
    pub num_inclusions: i64,
    pub inclusion_paths: HashMap<String, i64>,
}

#[derive(Debug)]
pub struct AnalysisResult {
    pub unit_analysis: HashMap<String, UnitAnalysis>,
    pub header_analysis: HashMap<String, HeaderAnalysis>,
    pub function_template_instantiations: HashMap<String, TemplateInstantiation>,
    pub class_template_instantiations: HashMap<String, TemplateInstantiation>,
}

impl AnalysisResult {
    pub fn from(unit_analysis: HashMap<String, UnitAnalysis>) -> Self {
        let header_analysis = Self::get_per_header_analysis(&unit_analysis);
        let (function_template_instantiations, class_template_instantiations) =
            Self::get_per_build_template_instantiations(&unit_analysis);
        AnalysisResult {
            unit_analysis,
            header_analysis,
            function_template_instantiations,
            class_template_instantiations,
        }
    }

    fn get_per_build_template_instantiations(
        per_unit_analysis: &HashMap<String, UnitAnalysis>,
    ) -> (
        HashMap<String, TemplateInstantiation>,
        HashMap<String, TemplateInstantiation>,
    ) {
        let mut function_template_instantiations: HashMap<String, TemplateInstantiation> =
            HashMap::new();
        let mut class_template_instantiations: HashMap<String, TemplateInstantiation> =
            HashMap::new();
        for analysis in per_unit_analysis.values() {
            for (name, inst) in &analysis.function_template_instantiations {
                function_template_instantiations
                    .entry(name.clone())
                    .and_modify(|val| val.merge(inst))
                    .or_insert_with(|| inst.clone());
            }
            for (name, inst) in &analysis.class_template_instantiations {
                class_template_instantiations
                    .entry(name.clone())
                    .and_modify(|val| val.merge(inst))
                    .or_insert_with(|| inst.clone());
            }
        }
        (
            function_template_instantiations,
            class_template_instantiations,
        )
    }

    pub fn longest_template_instantiations(
        &self,
        num_items: usize,
    ) -> impl Iterator<Item = (&str, &String, &TemplateInstantiation)> {
        let num_items = min(
            self.function_template_instantiations.len() + self.class_template_instantiations.len(),
            num_items,
        );
        self.function_template_instantiations
            .iter()
            .map(|(name, inst)| ("function", name, inst))
            .chain(
                self.class_template_instantiations
                    .iter()
                    .map(|(name, inst)| ("class", name, inst)),
            )
            .k_largest_by(num_items, |a, b| a.2.total_duration.cmp(&b.2.total_duration))
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
                        inclusion_paths: HashMap::new(),
                    });
            }
            for (header, chain_counts) in analysis.1.header_inclusion_paths.iter() {
                if let Some(header_analysis) = header_analysis.get_mut(header) {
                    for (chain, count) in chain_counts {
                        *header_analysis
                            .inclusion_paths
                            .entry(chain.clone())
                            .or_insert(0) += count;
                    }
                }
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
                template_instantiation_duration: 0,
                header_parse_duration: HashMap::new(),
                header_inclusion_paths: HashMap::new(),
                function_template_instantiations: HashMap::new(),
                class_template_instantiations: HashMap::new(),
            };
            let header_sources: Vec<(&String, &Vec<usize>)> = trace
                .trace_tree
                .roots
                .iter()
                .filter(|root| root.0 == SOURCE)
                .collect();
            for (_, roots) in header_sources {
                for root_id in roots {
                if let Some(node) = trace.trace_tree.nodes.get(*root_id) {
                        let header_and_dur = node
                            .trace
                            .args
                            .as_ref()
                            .and_then(|args| args.detail.as_deref())
                            .map(|header| header.splitn(2, ':').next().unwrap_or(header))
                            .filter(|h| h.ends_with(".h") || h.ends_with(".hpp"))
                            .map(|h| (h, node.trace.dur.unwrap_or(0)));
                        if let Some((header, duration)) = header_and_dur {
                            analysis
                                .header_parse_duration
                                .entry(header.to_string())
                                .and_modify(|val| *val += duration)
                                .or_insert(duration);
                        }
                    }
                }
            }
            let headers_parsed = !analysis.header_parse_duration.is_empty();
            if !headers_parsed {
                log::info!("Headers parsing duration was not extracted from 'Sources' traces. Computing manually");
            }

            // Returns true when the node is nested inside another instantiation scope.
            // Such nodes still count as individual template instantiations, but their
            // duration must not be added to `template_instantiation_duration`, otherwise
            // nested instantiations would be double counted.
            let has_instantiation_ancestor = |node: &TraceNode| -> bool {
                let mut parent = node.parent;
                while let Some(parent_id) = parent {
                    let parent_node = &trace.trace_tree.nodes[parent_id];
                    let parent_name = &parent_node.trace.name;
                    if parent_name == INSTANTIATE_FUNCTION
                        || parent_name == INSTANTIATE_CLASS
                        || parent_name == PERFORM_PENDING_INSTANTIATIONS
                    {
                        return true;
                    }
                    parent = parent_node.parent;
                }
                false
            };

            trace.trace_tree.traverse(|node| {
                if node.trace.dur.is_none() {
                    return LoopControl::Continue;
                }
                let duration = node.trace.dur.unwrap_or(0);
                let name = &node.trace.name;
                if name == TOTAL_EXECUTE_COMPILER {
                    analysis.total_duration = duration;
                    return LoopControl::Continue;
                }
                if name == TOTAL_FRONTEND {
                    analysis.frontend_duration = duration;
                    return LoopControl::Continue;
                }
                if name == TOTAL_BACKEND {
                    analysis.backend_duration = duration;
                    return LoopControl::Continue;
                }
                if name == INSTANTIATE_FUNCTION || name == INSTANTIATE_CLASS {
                    if let Some(template_name) = node
                        .trace
                        .args
                        .as_ref()
                        .and_then(|args| args.detail.as_deref())
                    {
                        let instantiations = if name == INSTANTIATE_FUNCTION {
                            &mut analysis.function_template_instantiations
                        } else {
                            &mut analysis.class_template_instantiations
                        };
                        instantiations
                            .entry(template_name.to_string())
                            .and_modify(|val| val.add(duration))
                            .or_insert_with(|| TemplateInstantiation::new(duration));
                    }
                    if !has_instantiation_ancestor(node) {
                        analysis.template_instantiation_duration += duration;
                    }
                    return LoopControl::Continue;
                }
                if name == PERFORM_PENDING_INSTANTIATIONS {
                    if !has_instantiation_ancestor(node) {
                        analysis.template_instantiation_duration += duration;
                    }
                    return LoopControl::Continue;
                }
                if name == PARSE_DECL_OR_FN_DEF && !headers_parsed {
                    if let Some(header) = node
                        .trace
                        .args
                        .as_ref()
                        .and_then(|args| args.detail.as_deref())
                    {
                        let header = header.splitn(2, ':').next().unwrap_or(header);
                        if header.ends_with(".h") || header.ends_with(".hpp") {
                            analysis
                                .header_parse_duration
                                .entry(header.to_string())
                                .and_modify(|val| *val += duration)
                                .or_insert(duration);
                        }
                    }
                    return LoopControl::Continue;
                }
                LoopControl::Continue
            });

            // Collect inclusion paths for headers from the "Source" event tree.
            // Clang emits a "Source" span (cat = "Source") for every entered file,
            // nested inside the file that includes it. The chain from the root down
            // to a header's "Source" node is therefore its full inclusion path.
            // The top-level source file is dropped so that the same header-only
            // chain (e.g. "a.h -> b.h") recurs across translation units and can be
            // counted as the most-used inclusion chain for the header.
            if let Some(source_root_ids) = trace.trace_tree.roots.get(SOURCE) {
                let mut stack: Vec<(usize, Vec<String>)> = source_root_ids
                    .iter()
                    .map(|&id| (id, Vec::new()))
                    .collect();
                while let Some((node_id, mut chain)) = stack.pop() {
                    let node = &trace.trace_tree.nodes[node_id];
                    let file = node
                        .trace
                        .args
                        .as_ref()
                        .and_then(|args| args.detail.as_deref())
                        .map(|f| f.splitn(2, ':').next().unwrap_or(f).to_string());
                    if let Some(file) = file {
                        chain.push(file.clone());
                        if file.ends_with(".h") || file.ends_with(".hpp") {
                            if chain.len() > 1 {
                                let header_chain = chain[1..].join(" -> ");
                                analysis
                                    .header_inclusion_paths
                                    .entry(file)
                                    .or_insert_with(HashMap::new)
                                    .entry(header_chain)
                                    .and_modify(|count| *count += 1)
                                    .or_insert(1);
                            }
                        }
                    }
                    for &child in &node.children {
                        stack.push((child, chain.clone()));
                    }
                }
            }
            (source_file, analysis)
        })
        .collect();
    AnalysisResult::from(unit_analysis)
}

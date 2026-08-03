use log;
use rayon::prelude::*;
use std::{collections::HashMap, fs::File, io::BufReader, path::PathBuf};

use crate::raw_trace::*;

fn parse_traces(build_dir: &str) -> HashMap<String, TraceEventFileRaw> {
    let pattern = build_dir.to_owned() + "/**/*.cpp.json";
    let paths: Vec<PathBuf> = match glob::glob(&pattern) {
        Ok(paths) => paths.filter_map(|e| e.ok()).collect(),
        Err(e) => {
            eprintln!("Glob pattern error: {}", e);
            return HashMap::new();
        }
    };
    log::trace!("Found {} trace files", paths.len());
    let parsed_traces: HashMap<String, TraceEventFileRaw> = paths
        .into_par_iter()
        .filter_map(|path| {
            let path_str = path.to_str()?.to_string();
            let file = File::open(&path).ok()?;
            let reader = BufReader::new(file);
            let trace = serde_json::from_reader(reader).ok()?;
            Some((path_str, trace))
        })
        .collect();
    log::trace!("Parsed {} trace files", parsed_traces.len());
    return parsed_traces;
}

#[derive(Debug)]
pub struct TraceNode {
    pub id: usize,
    pub trace: TraceEventRaw,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
}

#[derive(Debug, Default)]
pub struct TraceTree {
    pub root: Option<usize>,
    pub nodes: Vec<TraceNode>,
}

impl TraceTree {
    fn from(mut trace_events: Vec<TraceEventRaw>) -> Self {
        if trace_events.is_empty() {
            log::trace!("Empty trace events");
            return Self::default();
        }

        let execute_compiler_trace_pid = trace_events
            .iter()
            .find(|event| event.name == EXECUTE_COMPILER)
            .map(|event| event.pid);

        // Remove traces that do not have duration or that do not match the expected names
        trace_events.retain(|event| {
            if let Some(expected_pid) = execute_compiler_trace_pid {
                if event.pid != expected_pid {
                    return false;
                }
            }
            event.dur.is_some()
                && match event.name.as_str() {
                    EXECUTE_COMPILER
                    | FRONTEND
                    | BACKEND
                    | PARSE_DECL_OR_FN_DEF
                    | PARSE_FN_DEF
                    | PARSE_CLASS
                    | INSTANTIATE_CLASS
                    | INSTANTIATE_FUNCTION
                    | CODE_GEN_FUNCTION
                    | PERFORM_PENDING_INSTANTIATIONS
                    | OPTIMIZER
                    | CODE_GEN_PASSES
                    | OPT_MODULE => true,
                    _ => false,
                }
        });
        // Sort the events by the start time. If they match, place the longest duration first
        trace_events.sort_by(|a, b| a.ts.cmp(&b.ts).then_with(|| b.dur.cmp(&a.dur)));

        let mut tree = Self::default();

        // Initialize  nodes
        for (id, trace) in trace_events.into_iter().enumerate() {
            tree.nodes.push(TraceNode {
                id,
                trace,
                parent: None,
                children: Vec::new(),
            });
        }

        // The longest, earliest span becomes the root
        tree.root = Some(0);

        // Connect parents to children
        for i in 1..tree.nodes.len() {
            let mut potential_parent_idx = (i - 1) as isize;
            let current_trace = tree.nodes[i].trace.clone();
            let mut found_parent = false;

            // Search backwards for the closest parent that completely encloses this node
            while potential_parent_idx >= 0 {
                let p_idx = potential_parent_idx as usize;
                if tree.nodes[p_idx].trace.encloses(&current_trace) {
                    tree.nodes[i].parent = Some(p_idx);
                    tree.nodes[p_idx].children.push(i);
                    found_parent = true;
                    break;
                }
                potential_parent_idx -= 1;
            }

            // In case no parent completely encloses it, fallback to root as parent
            if !found_parent {
                tree.nodes[i].parent = Some(0);
                tree.nodes[0].children.push(i);
            }
        }

        tree
    }

    pub fn print_tree(&self, node_id: usize, depth: usize) {
        if let Some(node) = self.nodes.get(node_id) {
            let indent = " ".repeat(depth);
            println!("{}->{} ({})", indent, node.trace.name, node.trace.name);
            for &child_id in &node.children {
                self.print_tree(child_id, depth + 1);
            }
        }
    }
}

#[derive(Debug)]
pub struct FileTrace {
    pub trace_file: String,
    pub source_file: Option<String>,
    pub trace_tree: TraceTree,
}

pub fn collect_build_traces(build_dir: &str) -> Result<Vec<FileTrace>, String> {
    let parsed_traces = parse_traces(build_dir);
    let traces: Vec<FileTrace> = parsed_traces
        .into_par_iter()
        .filter_map(|parsed_trace| {
            let source_file = parsed_trace
                .1
                .trace_events
                .iter()
                .find(|&event| event.name == OPT_MODULE)
                .map(|event| match &event.args {
                    Some(args) => {
                        if let Some(detail) = args.get("detail") {
                            detail.to_string().trim_matches('"').to_string()
                        } else {
                            String::new()
                        }
                    }
                    None => String::new(),
                })
                .unwrap_or(String::new());
            Some(FileTrace {
                trace_file: parsed_trace.0,
                source_file: Some(source_file),
                trace_tree: TraceTree::from(parsed_trace.1.trace_events),
            })
        })
        .collect();
    Ok(traces)
}

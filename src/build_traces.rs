use itertools::Itertools;
use log;
use rayon::prelude::*;
use std::{collections::HashMap, fs::File, hash::Hash, io::BufReader, path::PathBuf};

use crate::raw_trace::*;

fn parse_traces(build_dir: &str) -> HashMap<String, TraceEventFile> {
    let pattern = build_dir.to_owned() + "/**/*.cpp.json";
    let paths: Vec<PathBuf> = match glob::glob(&pattern) {
        Ok(paths) => paths.filter_map(|e| e.ok()).collect(),
        Err(e) => {
            eprintln!("Glob pattern error: {}", e);
            return HashMap::new();
        }
    };
    log::trace!("Found {} trace files", paths.len());
    let parsed_traces: HashMap<String, TraceEventFile> = paths
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

type NodeId = usize;

#[derive(Debug)]
pub struct TraceNode {
    pub id: NodeId,
    pub trace: TraceEvent,
    pub parent: Option<NodeId>,
    pub children: Vec<NodeId>,
}

#[derive(Debug, Default)]
pub struct TraceTree {
    // There is one root for each PID found (key PID, value node ID)
    pub roots: HashMap<String, NodeId>,
    pub nodes: Vec<TraceNode>,
}

// Fuse async events of type "b" (begin) and "e" (end) into a single "X" (complete) event
fn fuse_async_trace_events(events: &mut Vec<TraceEvent>) {
    // Map tracking: (cat, id) -> index within the vector
    let mut pending_indices: HashMap<(Option<String>, i64), usize> = HashMap::new();
    // Track indices of "e" events that need to be deleted
    let mut indices_to_remove = Vec::new();

    for i in 0..events.len() {
        let ph = events[i].ph.as_str();

        if ph == "b" {
            if let Some(id) = events[i].id {
                let key = (events[i].cat.clone(), id);
                pending_indices.insert(key, i);
            }
        } else if ph == "e" {
            if let Some(ref id) = events[i].id {
                let key = (events[i].cat.clone(), id.clone());

                if let Some(start_idx) = pending_indices.remove(&key) {
                    let end_ts = events[i].ts;

                    // Mutate the original "b" event into an "X" event
                    let start_event = &mut events[start_idx];
                    start_event.ph = "X".to_string();
                    start_event.dur = Some(end_ts.saturating_sub(start_event.ts));

                    // Flag the current "e" event index for deletion
                    indices_to_remove.push(i);
                }
            }
        }
    }

    for &idx in indices_to_remove.iter().rev() {
        events.remove(idx);
    }
}

impl TraceTree {
    fn from(mut trace_events: Vec<TraceEvent>) -> Self {
        if trace_events.is_empty() {
            log::trace!("Empty trace events");
            return Self::default();
        }

        fuse_async_trace_events(&mut trace_events);

        // Sort the events by the category, then by process ID, then by the start time and then by the duration
        trace_events.sort_by(|a, b| {
            a.cat.cmp(&b.cat).then_with(|| {
                a.pid
                    .cmp(&b.pid)
                    .then_with(|| a.ts.cmp(&b.ts).then_with(|| b.dur.cmp(&a.dur)))
            })
        });

        let mut tree = Self::default();

        // Map to keep track of category root node indices by category name
        let mut category_roots: HashMap<String, usize> = HashMap::new();
        // Map to keep track of PID roots within each category: (category, pid) -> node_index
        let mut pid_roots: HashMap<(String, i64), usize> = HashMap::new();

        // Initialize  nodes
        for (id, trace) in trace_events.into_iter().enumerate() {
            tree.nodes.push(TraceNode {
                id,
                trace,
                parent: None,
                children: Vec::new(),
            });
        }

        // Connect parents to children. Each category will represent a different root, and each PID will have its own subtree
        for i in 1..tree.nodes.len() {
            let current_cat = tree.nodes[i].trace.cat.as_deref().unwrap_or("").to_string();
            let current_pid = tree.nodes[i].trace.pid;
            let current_trace = tree.nodes[i].trace.clone();

            let mut potential_parent_idx = (i as isize) - 1;
            let mut found_parent = false;

            // Search backwards for the closest parent with the same PID that encloses this node
            while potential_parent_idx >= 0 {
                let p_idx = potential_parent_idx as usize;
                let parent_node = &tree.nodes[p_idx];
                let parent_cat = parent_node.trace.cat.as_deref().unwrap_or("").to_string();

                if parent_cat == current_cat
                    && parent_node.trace.pid == current_pid
                    && parent_node.trace.encloses(&current_trace)
                {
                    tree.nodes[i].parent = Some(p_idx);
                    found_parent = true;
                    break;
                }
                potential_parent_idx -= 1;
            }

            if found_parent {
                if let Some(p_idx) = tree.nodes[i].parent {
                    tree.nodes[p_idx].children.push(i);
                }
            } else {
                // No enclosing parent found. This node is a root for its PID within this category.
                let cat_root_idx = if let Some(&c_idx) = category_roots.get(&current_cat) {
                    c_idx
                } else {
                    category_roots.insert(current_cat.clone(), i);
                    tree.roots.insert(current_cat.clone(), i);
                    i
                };

                if cat_root_idx == i {
                    // This node itself became the category root
                    continue;
                }

                // Check if a PID subtree root already exists under this category
                if let Some(&p_root_idx) = pid_roots.get(&(current_cat.clone(), current_pid)) {
                    tree.nodes[i].parent = Some(p_root_idx);
                    tree.nodes[p_root_idx].children.push(i);
                } else {
                    // First node seen with this PID under this category becomes the PID root
                    pid_roots.insert((current_cat, current_pid), i);

                    // Connect this PID root to the Category root
                    tree.nodes[i].parent = Some(cat_root_idx);
                    tree.nodes[cat_root_idx].children.push(i);
                }
            }
        }

        tree
    }

    fn print_children(&self, node_id: usize, depth: usize) {
        if let Some(node) = self.nodes.get(node_id) {
            let indent = " ".repeat(depth);
            println!(
                "{}->{} {}->{}us ({:?})",
                indent,
                node.trace.name,
                node.trace.ts,
                node.trace.ts + node.trace.dur.unwrap_or(0),
                node.trace.args
            );
            for &child_id in &node.children {
                self.print_children(child_id, depth + 1);
            }
        }
    }

    pub fn print_tree(&self, depth: usize) {
        for (cat, &root_id) in self.roots.iter() {
            println!("Category '{cat}'");
            self.print_children(root_id, depth);
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

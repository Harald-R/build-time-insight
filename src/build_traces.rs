use log;
use rayon::prelude::*;
use rustc_hash::{FxHashMap, FxHashSet};
use std::{collections::HashMap, fs::File, io::BufReader, path::PathBuf};

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
    // There can be multiple roots for each category found (key PID, value node IDs)
    pub roots: FxHashMap<String, Vec<NodeId>>,
    pub nodes: Vec<TraceNode>,
}

// Fuse async events of type "b" (begin) and "e" (end) into a single "X" (complete) event
fn fuse_async_trace_events(events: &mut Vec<TraceEvent>) {
    // Map tracking: (cat, id) -> index within the vector
    let mut pending_indices: FxHashMap<(Option<&str>, i64), usize> =
        FxHashMap::with_capacity_and_hasher(events.len() / 2, Default::default());
    let mut mutations = Vec::with_capacity(events.len() / 4);
    let mut indices_to_remove =
        FxHashSet::with_capacity_and_hasher(events.len() / 2, Default::default());

    for i in 0..events.len() {
        let ph = events[i].ph.as_str();

        if ph == "b" {
            if let Some(id) = events[i].id {
                let key = (events[i].cat.as_deref(), id);
                pending_indices.insert(key, i);
            }
        } else if ph == "e" {
            if let Some(id) = events[i].id {
                let key = (events[i].cat.as_deref(), id);

                if let Some(start_idx) = pending_indices.remove(&key) {
                    let end_ts = events[i].ts;
                    mutations.push((start_idx, end_ts));
                    indices_to_remove.insert(i);
                }
            }
        }
    }

    for (start_idx, end_ts) in mutations {
        let start_event = &mut events[start_idx];
        start_event.ph = "X".to_string();
        start_event.dur = Some(end_ts.saturating_sub(start_event.ts));
    }

    let mut current_idx = 0;
    events.retain(|_| {
        let keep = !indices_to_remove.contains(&current_idx);
        current_idx += 1;
        keep
    });
}

impl TraceTree {
    pub fn from(mut trace_events: Vec<TraceEvent>) -> Self {
        if trace_events.is_empty() {
            log::trace!("Empty trace events");
            return Self::default();
        }

        // Combine matching separate begin "b" and end "e" events into single duration "X" events
        fuse_async_trace_events(&mut trace_events);

        // Group sequentially by thread (cat -> pid -> tid) then sort chronologically (ts)
        // with longest durations (dur) first to ensure parents precede children
        trace_events.sort_by(|a, b| {
            a.cat.cmp(&b.cat).then_with(|| {
                a.pid.cmp(&b.pid).then_with(|| {
                    a.tid
                        .cmp(&b.tid)
                        .then_with(|| a.ts.cmp(&b.ts).then_with(|| b.dur.cmp(&a.dur)))
                })
            })
        });

        let mut tree = TraceTree {
            roots: FxHashMap::default(),
            nodes: Vec::with_capacity(trace_events.len()),
        };

        for (id, trace) in trace_events.into_iter().enumerate() {
            tree.nodes.push(TraceNode {
                id,
                trace,
                parent: None,
                children: Vec::new(),
            });
        }

        // Key is (category, pid, tid) to isolate call stacks by exact execution threads
        type ThreadKey<'a> = (Option<&'a str>, i64, i64);
        let mut thread_lanes: FxHashMap<ThreadKey, Vec<usize>> = FxHashMap::default();

        // Stores (child_idx, parent_idx) relationships
        let mut parent_child_links = Vec::with_capacity(tree.nodes.len());

        for i in 0..tree.nodes.len() {
            let is_metadata = tree.nodes[i].trace.ph == "M";

            let current_cat_str = tree.nodes[i].trace.cat.as_deref();
            let current_pid = tree.nodes[i].trace.pid;
            let current_tid = tree.nodes[i].trace.tid;

            let key: ThreadKey = (current_cat_str, current_pid, current_tid);
            let active_stack = thread_lanes.entry(key).or_insert_with(Vec::new);

            let mut found_parent = false;
            let mut matched_p_idx = 0;
            let current_ts = tree.nodes[i].trace.ts;

            // Prune expired scopes from the stack and search backwards for an enclosing parent
            while let Some(&p_idx) = active_stack.last() {
                let parent_trace = &tree.nodes[p_idx].trace;
                let parent_end = parent_trace.ts + parent_trace.dur.unwrap_or(0);

                if current_ts < parent_end {
                    // Current parent is still alive. Check if it encloses the node
                    if parent_trace.encloses(&tree.nodes[i].trace) {
                        matched_p_idx = p_idx;
                        found_parent = true;
                    }
                    break;
                }
                // Sibling or past parent interval has completely expired, drop it from the stack
                active_stack.pop();
            }

            if found_parent {
                parent_child_links.push((i, matched_p_idx));
            }

            // Only register this event as a parent if it has a real structural duration container
            let can_be_parent = !is_metadata && tree.nodes[i].trace.dur.map_or(false, |d| d > 0);
            if can_be_parent {
                active_stack.push(i);
            }

            // If the node has a structural parent inside its lane, we skip root assignment
            if found_parent {
                continue;
            }

            // Node is not enclosed by any other event; it becomes a root in its category
            let root_name = current_cat_str.unwrap_or("").to_string();
            tree.roots.entry(root_name).or_insert_with(Vec::new).push(i);
        }

        // Apply parents and populate children arrays
        for (child_idx, parent_idx) in parent_child_links {
            tree.nodes[child_idx].parent = Some(parent_idx);
            tree.nodes[parent_idx].children.push(child_idx);
        }

        tree
    }

    fn print_children(&self, node_id: usize, depth: usize) {
        if let Some(node) = self.nodes.get(node_id) {
            let indent = " ".repeat(depth);
            println!(
                "{}{}->{} {}->{}us ({:?})",
                depth,
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
        for (cat, roots) in self.roots.iter() {
            println!("Category '{cat}'");
            for root_id in roots {
                self.print_children(*root_id, depth);
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
                        if let Some(detail) = args.get(DETAIL) {
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

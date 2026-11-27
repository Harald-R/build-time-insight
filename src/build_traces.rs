//! Parses `-ftime-trace` JSON files into an event tree.
use log;
use rustc_hash::{FxHashMap, FxHashSet};
use std::{
    fs::File,
    io::{BufReader, Read},
    path::{Path, PathBuf},
};

use crate::raw_trace::*;

fn deserialize_trace_event_file<R: Read>(reader: R) -> Result<TraceEventFile, serde_json::Error> {
    serde_json::from_reader(reader)
}

/// Glob the build directory for `-ftime-trace` files, skipping any that match
/// one of the exclude patterns (matched relative to `build_dir`). A pattern
/// matching a parent directory also excludes everything below it.
pub fn glob_trace_paths(build_dir: &str, excludes: &[String]) -> Vec<PathBuf> {
    let exclude_patterns: Vec<glob::Pattern> = excludes
        .iter()
        .filter_map(|pattern| match glob::Pattern::new(pattern) {
            Ok(pattern) => Some(pattern),
            Err(e) => {
                log::warn!("Invalid exclude pattern '{}': {}", pattern, e);
                None
            }
        })
        .collect();
    let is_excluded = |path: &Path| {
        let relative = path.strip_prefix(build_dir).unwrap_or(path);
        exclude_patterns.iter().any(|p| {
            relative
                .ancestors()
                .any(|ancestor| p.matches_path(ancestor))
        })
    };
    let pattern = build_dir.to_owned() + "/**/*.json";
    let mut paths = Vec::new();
    match glob::glob(&pattern) {
        Ok(entries) => {
            for entry in entries {
                match entry {
                    Ok(path) => {
                        if is_excluded(&path) {
                            log::debug!("Excluding '{}'", path.display());
                            continue;
                        }
                        // CMake emits metadata JSON next to the traces. Accept
                        // the conventional names without opening them, and
                        // sniff other files for a trace payload (clang <= 21
                        // names module-interface phase traces after the PCM).
                        if is_cmake_probe_trace(build_dir, &path) {
                            log::debug!("Ignoring CMake probe trace '{}'", path.display());
                            continue;
                        }
                        let known_trace_name = path
                            .file_name()
                            .and_then(|name| name.to_str())
                            .is_some_and(|name| {
                                name.ends_with(".cpp.json") || name.ends_with(".cppm.json")
                            });
                        if known_trace_name || is_trace_json(&path) {
                            paths.push(path);
                        }
                    }
                    Err(e) => {
                        log::warn!("Error while globbing '{}': {}", pattern, e);
                    }
                }
            }
        }
        Err(e) => {
            log::error!("Glob pattern error for '{}': {}", pattern, e);
        }
    }
    log::trace!("Found {} trace files", paths.len());
    paths
}

/// CMake's compiler/ABI probes can emit `-ftime-trace` files when the flags
/// are set globally. They live under `CMakeFiles/<cmake-version>/` or
/// `CMakeFiles/CMakeScratch/` and are not project translation units.
fn is_cmake_probe_trace(build_dir: &str, path: &Path) -> bool {
    let rel = path.strip_prefix(build_dir).unwrap_or(path);
    let mut components = rel.components();
    let Some(first) = components.next() else {
        return false;
    };
    if first.as_os_str() != "CMakeFiles" {
        return false;
    }
    let Some(second) = components.next() else {
        return false;
    };
    let name = second.as_os_str().to_string_lossy();
    name == "CMakeScratch" || name.starts_with(|c: char| c.is_ascii_digit())
}

/// Returns true when the file starts with a clang trace payload.
fn is_trace_json(path: &Path) -> bool {
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let mut probe = [0u8; 512];
    let Ok(read) = file.read(&mut probe) else {
        return false;
    };
    String::from_utf8_lossy(&probe[..read]).contains("\"traceEvents\"")
}

/// Parse a trace file into its raw events, logging and skipping broken files
pub fn parse_trace_file(path: &Path) -> Option<TraceEventFile> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(e) => {
            log::warn!("Could not open trace file '{}': {}", path.display(), e);
            return None;
        }
    };
    let reader = BufReader::new(file);
    match deserialize_trace_event_file(reader) {
        Ok(trace) => Some(trace),
        Err(e) => {
            log::warn!("Skipping broken trace file '{}': {}", path.display(), e);
            None
        }
    }
}

/// Extract the translation unit's source file from the `OptModule` event
pub fn source_file_from_trace(trace: &TraceEventFile) -> String {
    trace
        .trace_events
        .iter()
        .find(|&event| event.name == OPT_MODULE)
        .and_then(|event| event.args.as_ref().and_then(|args| args.detail.as_deref()))
        .unwrap_or("")
        .to_string()
}

/// Extract the PCM/BMI path associated with a module-unit trace. Clang <= 21
/// emits separate phase traces named after this file, while `OptModule` in the
/// module-unit trace points at the PCM rather than the `.cppm` source.
pub fn pcm_file_from_trace(trace: &TraceEventFile) -> Option<String> {
    trace
        .trace_events
        .iter()
        .filter(|event| event.name == OPT_MODULE || event.name == "WriteAST")
        .filter_map(|event| event.args.as_ref().and_then(|args| args.detail.as_deref()))
        .find(|detail| detail.ends_with(".pcm"))
        .map(str::to_string)
}

type NodeId = usize;

/// A single node in a build trace tree.
#[derive(Debug)]
pub struct TraceNode {
    /// Unique index of the node in the tree.
    pub id: NodeId,
    /// Trace event that this node represents.
    pub trace: TraceEvent,
    /// Index of the parent node, if present.
    pub parent: Option<NodeId>,
    /// Indexes of the child nodes.
    pub children: Vec<NodeId>,
}

/// A tree that groups trace events by category.
#[derive(Debug, Default)]
pub struct TraceTree {
    // There can be multiple roots for each category found (key PID, value node IDs)
    /// Root node indexes for each category.
    pub roots: FxHashMap<String, Vec<NodeId>>,
    /// All nodes in the tree.
    pub nodes: Vec<TraceNode>,
}

/// Control signal returned by your closure to guide the tree traversal loop
pub enum LoopControl {
    /// Continue down the tree normally
    Continue,
    /// Skip this node's children, but keep checking other branches/siblings
    SkipChildren,
    /// Abort the entire traversal immediately
    Break,
}

// Clang 18 and older emit "Source" spans as complete "X" events without a
// category; clang 19+ emits them as async begin/end events in the "Source"
// category. Normalize the older shape so tree building and header extraction
// see the same category.
fn normalize_source_category(events: &mut [TraceEvent]) {
    for event in events {
        if event.name == SOURCE && event.cat.as_deref().is_none_or(str::is_empty) {
            event.cat = Some(SOURCE.to_string());
        }
    }
}

// Fuse async events of type "b" (begin) and "e" (end) into a single "X" (complete) event
fn fuse_async_trace_events(events: &mut Vec<TraceEvent>) {
    // Stack per (cat, id) of unclosed begin indices within the vector.
    // Nested async spans share one key, so a LIFO stack pairs each end with the
    // most recent unclosed begin, whether the file order is consecutive (b,e,b,e)
    // or nested (b,b,e,e).
    let mut pending_stacks: FxHashMap<(Option<&str>, i64), Vec<usize>> =
        FxHashMap::with_capacity_and_hasher(events.len() / 2, Default::default());
    let mut mutations = Vec::with_capacity(events.len() / 4);
    let mut indices_to_remove =
        FxHashSet::with_capacity_and_hasher(events.len() / 2, Default::default());

    for (i, event) in events.iter().enumerate() {
        let ph = event.ph.as_str();

        if ph == "b" {
            if let Some(id) = event.id {
                let key = (event.cat.as_deref(), id);
                pending_stacks.entry(key).or_default().push(i);
            }
        } else if ph == "e"
            && let Some(id) = event.id
        {
            let key = (event.cat.as_deref(), id);

            if let Some(start_idx) = pending_stacks.get_mut(&key).and_then(|s| s.pop()) {
                let end_ts = event.ts;
                mutations.push((start_idx, end_ts));
                indices_to_remove.insert(i);
            }
        }
    }

    for (start_idx, end_ts) in mutations {
        let start_event = &mut events[start_idx];
        start_event.ph = "X".to_string();
        start_event.dur = Some(end_ts.saturating_sub(start_event.ts).max(0));
    }

    let mut current_idx = 0;
    events.retain(|_| {
        let keep = !indices_to_remove.contains(&current_idx);
        current_idx += 1;
        keep
    });
}

impl TraceTree {
    /// Build a tree from the given trace events.
    pub fn from(mut trace_events: Vec<TraceEvent>) -> Self {
        if trace_events.is_empty() {
            log::trace!("Empty trace events");
            return Self::default();
        }

        // Clang 18 represents "Source" spans differently from clang 19+.
        normalize_source_category(&mut trace_events);

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
            let active_stack = thread_lanes.entry(key).or_default();

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
            let can_be_parent = !is_metadata && tree.nodes[i].trace.dur.is_some_and(|d| d > 0);
            if can_be_parent {
                active_stack.push(i);
            }

            // If the node has a structural parent inside its lane, we skip root assignment
            if found_parent {
                continue;
            }

            // Node is not enclosed by any other event; it becomes a root in its category
            let root_name = current_cat_str.unwrap_or("").to_string();
            tree.roots.entry(root_name).or_default().push(i);
        }

        // Apply parents and populate children arrays
        for (child_idx, parent_idx) in parent_child_links {
            tree.nodes[child_idx].parent = Some(parent_idx);
            tree.nodes[parent_idx].children.push(child_idx);
        }

        tree
    }

    /// Visit each node with the given closure.
    pub fn traverse<F>(&self, mut f: F)
    where
        F: FnMut(&TraceNode) -> LoopControl,
    {
        let mut stack = Vec::new();

        for root_ids in self.roots.values() {
            for &root_id in root_ids.iter().rev() {
                stack.push(root_id);
            }
        }

        while let Some(node_id) = stack.pop() {
            if let Some(node) = self.nodes.get(node_id) {
                match f(node) {
                    LoopControl::Break => {
                        // Stop the traversal
                        break;
                    }
                    LoopControl::SkipChildren => {
                        // Skip pushing the children, so the loop moves straight to the next sibling in the stack
                    }
                    LoopControl::Continue => {
                        // Push children in reverse order so the first child is popped next
                        for &child_id in node.children.iter().rev() {
                            stack.push(child_id);
                        }
                    }
                }
            }
        }
    }

    fn print_children(&self, node_id: usize, depth: usize) {
        if let Some(node) = self.nodes.get(node_id) {
            let indent = " ".repeat(depth);
            log::trace!(
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

    /// Print the tree at the given depth.
    pub fn print_tree(&self, depth: usize) {
        for (cat, roots) in self.roots.iter() {
            log::trace!("Category '{cat}'");
            for root_id in roots {
                self.print_children(*root_id, depth);
            }
        }
    }
}

/// Trace data for a single translation unit.
#[derive(Debug)]
pub struct FileTrace {
    /// Path to the trace file.
    pub trace_file: String,
    /// Path to the source file, if known.
    pub source_file: Option<String>,
    /// Tree built from the trace file.
    pub trace_tree: TraceTree,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    // Build a raw event
    #[allow(clippy::too_many_arguments)]
    fn trace_event(
        name: &str,
        cat: Option<&str>,
        ph: &str,
        pid: i64,
        tid: i64,
        id: Option<i64>,
        ts: i64,
        dur: Option<i64>,
        detail: Option<&str>,
    ) -> TraceEvent {
        TraceEvent {
            name: name.to_string(),
            cat: cat.map(str::to_string),
            ph: ph.to_string(),
            pid,
            tid,
            id,
            ts,
            dur,
            args: detail.map(|d| TraceArgs {
                detail: Some(d.to_string()),
            }),
        }
    }

    // Complete "X" event as emitted by clang: no category, pid == tid, no args
    fn x_event(name: &str, pid: i64, tid: i64, ts: i64, dur: i64) -> TraceEvent {
        trace_event(name, None, "X", pid, tid, None, ts, Some(dur), None)
    }

    // Complete "X" event with an args.detail payload (e.g. a header location)
    fn x_event_detail(
        name: &str,
        pid: i64,
        tid: i64,
        ts: i64,
        dur: i64,
        detail: &str,
    ) -> TraceEvent {
        trace_event(name, None, "X", pid, tid, None, ts, Some(dur), Some(detail))
    }

    // Metadata event (process_name / thread_name): empty category, no duration
    fn metadata_event(name: &str, pid: i64, tid: i64) -> TraceEvent {
        trace_event(name, Some(""), "M", pid, tid, None, 0, None, None)
    }

    // Async "Source" begin event: category "Source", id 0, detail = included file
    fn source_begin(pid: i64, tid: i64, ts: i64, file: &str) -> TraceEvent {
        trace_event(
            "Source",
            Some("Source"),
            "b",
            pid,
            tid,
            Some(0),
            ts,
            None,
            Some(file),
        )
    }

    // Async "Source" end event: category "Source", id 0, no args
    fn source_end(pid: i64, tid: i64, ts: i64) -> TraceEvent {
        trace_event(
            "Source",
            Some("Source"),
            "e",
            pid,
            tid,
            Some(0),
            ts,
            None,
            None,
        )
    }

    fn test_build_dir(label: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("build_time_insight_{label}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_trace_file(dir: &Path, rel_path: &str, content: &str) {
        let path = dir.join(rel_path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    #[test]
    fn fuse_async_trace_events_combines_begin_end_into_complete() {
        let mut events = vec![
            source_begin(1, 1, 100, "/usr/include/cdefs.h"),
            source_end(1, 1, 250),
        ];
        fuse_async_trace_events(&mut events);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].ph, "X");
        assert_eq!(events[0].name, "Source");
        assert_eq!(events[0].cat.as_deref(), Some("Source"));
        assert_eq!(events[0].ts, 100);
        assert_eq!(events[0].dur, Some(150));
        assert_eq!(
            events[0].args.as_ref().unwrap().detail.as_deref(),
            Some("/usr/include/cdefs.h")
        );
    }

    #[test]
    fn fuse_async_trace_events_handles_nested_source_events() {
        let mut events = vec![
            source_begin(1, 1, 100, "/usr/include/cdefs.h"),
            source_end(1, 1, 200),
            source_begin(1, 1, 110, "/usr/include/features.h"),
            source_end(1, 1, 150),
        ];
        fuse_async_trace_events(&mut events);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].ts, 100);
        assert_eq!(events[0].dur, Some(100));
        assert_eq!(
            events[0].args.as_ref().unwrap().detail.as_deref(),
            Some("/usr/include/cdefs.h")
        );
        assert_eq!(events[1].ts, 110);
        assert_eq!(events[1].dur, Some(40));
        assert_eq!(
            events[1].args.as_ref().unwrap().detail.as_deref(),
            Some("/usr/include/features.h")
        );
    }

    #[test]
    fn fuse_async_trace_events_handles_nested_file_order() {
        let mut events = vec![
            source_begin(1, 1, 100, "/usr/include/cdefs.h"),
            source_begin(1, 1, 110, "/usr/include/features.h"),
            source_end(1, 1, 150),
            source_end(1, 1, 200),
        ];
        fuse_async_trace_events(&mut events);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].ts, 100);
        assert_eq!(events[0].dur, Some(100));
        assert_eq!(
            events[0].args.as_ref().unwrap().detail.as_deref(),
            Some("/usr/include/cdefs.h")
        );
        assert_eq!(events[1].ts, 110);
        assert_eq!(events[1].dur, Some(40));
        assert_eq!(
            events[1].args.as_ref().unwrap().detail.as_deref(),
            Some("/usr/include/features.h")
        );
    }

    #[test]
    fn fuse_async_trace_events_leaves_unpaired_events_untouched() {
        // The end event is before the beginning event
        let mut events = vec![
            source_end(1, 1, 200),
            source_begin(1, 1, 100, "/usr/include/cdefs.h"),
        ];
        fuse_async_trace_events(&mut events);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].ph, "e");
        assert_eq!(events[0].dur, None);
        assert_eq!(events[1].ph, "b");
        assert_eq!(events[1].dur, None);
    }

    #[test]
    fn fuse_async_trace_events_ignores_events_without_id() {
        let mut events = vec![
            trace_event(
                "Source",
                Some("Source"),
                "b",
                1,
                1,
                None,
                100,
                None,
                Some("a.h"),
            ),
            trace_event("Source", Some("Source"), "e", 1, 1, None, 200, None, None),
        ];
        fuse_async_trace_events(&mut events);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].ph, "b");
        assert_eq!(events[1].ph, "e");
    }

    #[test]
    fn fuse_async_trace_events_saturates_duration_when_end_precedes_begin() {
        let mut events = vec![
            source_begin(1, 1, 100, "/usr/include/cdefs.h"),
            source_end(1, 1, 50),
        ];
        fuse_async_trace_events(&mut events);
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].ph, "X");
        assert_eq!(events[0].dur, Some(0));
    }

    #[test]
    fn empty_trace_events_yield_default_tree() {
        let tree = TraceTree::from(Vec::new());
        assert!(tree.roots.is_empty());
        assert!(tree.nodes.is_empty());
    }

    #[test]
    fn nested_events_form_parent_child_links() {
        let events = vec![
            x_event("Frontend", 1, 1, 0, 100),
            x_event_detail(
                "ParseDeclarationOrFunctionDefinition",
                1,
                1,
                10,
                50,
                "src/main.cpp:12:3",
            ),
            x_event("ParseFunctionDefinition", 1, 1, 20, 10),
        ];
        let tree = TraceTree::from(events);

        assert_eq!(tree.roots[""], vec![0]);
        let root = &tree.nodes[0];
        assert_eq!(root.trace.name, "Frontend");
        assert_eq!(root.parent, None);
        assert_eq!(root.children, vec![1]);

        assert_eq!(
            tree.nodes[1].trace.name,
            "ParseDeclarationOrFunctionDefinition"
        );
        assert_eq!(tree.nodes[1].parent, Some(0));
        assert_eq!(tree.nodes[1].children, vec![2]);

        assert_eq!(tree.nodes[2].trace.name, "ParseFunctionDefinition");
        assert_eq!(tree.nodes[2].parent, Some(1));
        assert!(tree.nodes[2].children.is_empty());
    }

    #[test]
    fn events_on_different_threads_are_isolated() {
        // The "Total ..." events are emitted on their own threads in real traces
        let events = vec![
            x_event("ExecuteCompiler", 1, 1, 100, 5000),
            x_event("Total ExecuteCompiler", 1, 2, 0, 4999),
        ];
        let tree = TraceTree::from(events);
        assert_eq!(tree.roots[""].len(), 2);
        assert!(tree.nodes.iter().all(|n| n.parent.is_none()));
    }

    #[test]
    fn events_in_different_categories_become_separate_roots() {
        let events = vec![
            x_event("ExecuteCompiler", 1, 1, 100, 5000),
            trace_event(
                "Source",
                Some("Source"),
                "X",
                1,
                1,
                None,
                1000,
                Some(500),
                Some("src/main.cpp"),
            ),
        ];
        let tree = TraceTree::from(events);
        assert_eq!(tree.roots[""], vec![0]);
        assert_eq!(tree.roots["Source"], vec![1]);
        assert!(tree.nodes.iter().all(|n| n.parent.is_none()));
    }

    #[test]
    fn uncategorized_source_events_are_grouped_under_source() {
        // clang 18 emits complete "Source" events without a category.
        let events = vec![
            x_event("Frontend", 1, 1, 0, 500),
            x_event_detail("Source", 1, 1, 100, 200, "/usr/include/cdefs.h"),
            x_event_detail("Source", 1, 1, 110, 50, "/usr/include/features.h"),
        ];
        let tree = TraceTree::from(events);

        assert_eq!(tree.roots[""], vec![0]);
        assert_eq!(tree.roots[SOURCE], vec![1]);
        assert_eq!(tree.nodes[1].children, vec![2]);
        assert_eq!(tree.nodes[2].parent, Some(1));
    }

    #[test]
    fn zero_duration_events_cannot_be_parents() {
        let events = vec![x_event("a", 1, 1, 0, 0), x_event("b", 1, 1, 0, 0)];
        let tree = TraceTree::from(events);
        assert_eq!(tree.roots[""].len(), 2);
        assert!(tree.nodes.iter().all(|n| n.parent.is_none()));
    }

    #[test]
    fn metadata_events_do_not_act_as_parents() {
        // process_name/thread_name events have no duration and stay separate roots
        let events = vec![
            metadata_event("process_name", 1, 1),
            x_event("ExecuteCompiler", 1, 1, 0, 100),
        ];
        let tree = TraceTree::from(events);
        assert_eq!(tree.roots[""].len(), 2);
        assert!(tree.nodes.iter().all(|n| n.parent.is_none()));
    }

    #[test]
    fn nested_source_spans_form_parent_child_links() {
        let mut events = vec![
            source_begin(1, 1, 100, "/usr/include/cdefs.h"),
            source_end(1, 1, 200),
            source_begin(1, 1, 110, "/usr/include/features.h"),
            source_end(1, 1, 150),
        ];
        fuse_async_trace_events(&mut events);
        let tree = TraceTree::from(events);
        // Both spans share the "Source" category, so the enclosing span is the root
        assert_eq!(tree.roots["Source"], vec![0]);
        assert_eq!(tree.nodes[0].trace.name, "Source");
        assert_eq!(tree.nodes[0].children, vec![1]);
        assert_eq!(tree.nodes[1].parent, Some(0));
        assert_eq!(
            tree.nodes[0].trace.args.as_ref().unwrap().detail.as_deref(),
            Some("/usr/include/cdefs.h")
        );
    }

    #[test]
    fn partially_overlapping_events_are_not_nested() {
        let events = vec![x_event("a", 1, 1, 0, 10), x_event("b", 1, 1, 5, 10)];
        let tree = TraceTree::from(events);
        assert_eq!(tree.roots[""].len(), 2);
        assert!(tree.nodes.iter().all(|n| n.parent.is_none()));
    }

    fn sample_tree() -> TraceTree {
        TraceTree::from(vec![
            x_event("ExecuteCompiler", 1, 1, 0, 100),
            x_event("Frontend", 1, 1, 10, 50),
            x_event("ParseFunctionDefinition", 1, 1, 11, 5),
            x_event("Backend", 1, 1, 70, 20),
        ])
    }

    #[test]
    fn traverse_visits_nodes_in_depth_first_order() {
        let tree = sample_tree();
        let mut visited = Vec::new();
        tree.traverse(|node| {
            visited.push(node.trace.name.clone());
            LoopControl::Continue
        });
        assert_eq!(
            visited,
            vec![
                "ExecuteCompiler",
                "Frontend",
                "ParseFunctionDefinition",
                "Backend"
            ]
        );
    }

    #[test]
    fn traverse_skip_children_skips_subtree() {
        let tree = sample_tree();
        let mut visited = Vec::new();
        tree.traverse(|node| {
            visited.push(node.trace.name.clone());
            if node.trace.name == "Frontend" {
                LoopControl::SkipChildren
            } else {
                LoopControl::Continue
            }
        });
        assert_eq!(visited, vec!["ExecuteCompiler", "Frontend", "Backend"]);
    }

    #[test]
    fn traverse_break_aborts_traversal() {
        let tree = sample_tree();
        let mut visited = Vec::new();
        tree.traverse(|node| {
            visited.push(node.trace.name.clone());
            if node.trace.name == "Frontend" {
                LoopControl::Break
            } else {
                LoopControl::Continue
            }
        });
        assert_eq!(visited, vec!["ExecuteCompiler", "Frontend"]);
    }

    #[test]
    fn deserialize_trace_event_file_parses_valid_json() {
        let json = r#"{
            "traceEvents": [
                {
                    "pid": 1, "tid": 1, "ts": 5663257, "ph": "X", "dur": 731025,
                    "name": "OptModule",
                    "args": { "detail": "src/main.cpp" }
                },
                {
                    "pid": 1, "tid": 1, "ts": 0, "ph": "M", "cat": "",
                    "name": "process_name",
                    "args": { "name": "clang-22" }
                }
            ],
            "beginningOfTime": 1785441886055364
        }"#;
        let file = deserialize_trace_event_file(json.as_bytes()).unwrap();
        assert_eq!(file.beginning_of_time, 1785441886055364);
        assert_eq!(file.trace_events.len(), 2);

        let opt_module = &file.trace_events[0];
        assert_eq!(opt_module.name, "OptModule");
        assert_eq!(opt_module.cat, None);
        assert_eq!(opt_module.ph, "X");
        assert_eq!(opt_module.pid, 1);
        assert_eq!(opt_module.tid, 1);
        assert_eq!(opt_module.ts, 5663257);
        assert_eq!(opt_module.dur, Some(731025));
        assert_eq!(
            opt_module.args.as_ref().unwrap().detail.as_deref(),
            Some("src/main.cpp")
        );

        let process_name = &file.trace_events[1];
        assert_eq!(process_name.name, "process_name");
        assert_eq!(process_name.cat.as_deref(), Some(""));
        assert_eq!(process_name.dur, None);
        // clang's args = { "name": ... } is not consumed by TraceArgs
        assert!(process_name.args.as_ref().unwrap().detail.is_none());
    }

    #[test]
    fn deserialize_trace_event_file_defaults_missing_optional_fields() {
        let json = r#"{
            "traceEvents": [
                { "name": "Frontend", "ph": "X", "pid": 1, "tid": 1, "ts": 2833, "dur": 2819761 }
            ]
        }"#;
        let file: TraceEventFile = deserialize_trace_event_file(json.as_bytes()).unwrap();
        assert_eq!(file.beginning_of_time, 0);
        let event = &file.trace_events[0];
        assert_eq!(event.cat, None);
        assert_eq!(event.id, None);
        assert_eq!(event.dur, Some(2819761));
        assert!(event.args.is_none());
    }

    #[test]
    fn deserialize_trace_event_file_rejects_invalid_json() {
        assert!(deserialize_trace_event_file("not json".as_bytes()).is_err());
    }

    #[test]
    fn glob_trace_paths_finds_cpp_and_cppm_json_files() {
        let dir = test_build_dir("glob_trace_paths");
        write_trace_file(&dir, "main.cpp.json", "{}");
        write_trace_file(&dir, "nested/second.cpp.json", "{}");
        write_trace_file(&dir, "module.cppm.json", "{}");
        write_trace_file(&dir, "core.json", r#"{ "traceEvents": [] }"#);
        write_trace_file(
            &dir,
            "CMakeFiles/4.4.4/CompilerIdCXX/a-CMakeCXXCompilerId.json",
            r#"{ "traceEvents": [] }"#,
        );
        write_trace_file(&dir, "CXXModules.json", "{}");
        write_trace_file(&dir, "ignored.json", "{}");
        write_trace_file(&dir, "notes.txt", "{}");

        let paths = glob_trace_paths(dir.to_str().unwrap(), &[]);
        std::fs::remove_dir_all(&dir).ok();

        assert_eq!(paths.len(), 4);
        let names: Vec<String> = paths
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"main.cpp.json".to_string()));
        assert!(names.contains(&"second.cpp.json".to_string()));
        assert!(names.contains(&"module.cppm.json".to_string()));
        assert!(names.contains(&"core.json".to_string()));
    }

    #[test]
    fn glob_trace_paths_excludes_matching_files() {
        let dir = test_build_dir("glob_trace_paths_excludes");
        write_trace_file(&dir, "main.cpp.json", "{}");
        write_trace_file(&dir, "third_party/external.cpp.json", "{}");
        write_trace_file(&dir, "tests/test.cpp.json", "{}");

        let paths = glob_trace_paths(dir.to_str().unwrap(), &["third_party".into()]);
        std::fs::remove_dir_all(&dir).ok();

        let names: Vec<String> = paths
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["main.cpp.json", "test.cpp.json"]);
    }

    #[test]
    fn parse_trace_file_parses_valid_json_and_ignores_invalid() {
        let dir = test_build_dir("parse_trace_file");
        write_trace_file(
            &dir,
            "main.cpp.json",
            r#"{ "traceEvents": [], "beginningOfTime": 1785441886055364 }"#,
        );
        write_trace_file(&dir, "broken.cpp.json", "{ not valid json");

        assert!(parse_trace_file(&dir.join("main.cpp.json")).is_some());
        assert!(parse_trace_file(&dir.join("broken.cpp.json")).is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn source_file_from_trace_extracts_opt_module_detail() {
        let trace = TraceEventFile {
            trace_events: vec![trace_event(
                "OptModule",
                None,
                "X",
                1,
                1,
                None,
                0,
                Some(10),
                Some("src/main.cpp"),
            )],
            beginning_of_time: 0,
        };
        assert_eq!(source_file_from_trace(&trace), "src/main.cpp");
    }
}

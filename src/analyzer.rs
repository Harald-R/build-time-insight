use std::cmp::min;
use std::collections::{BTreeMap, HashMap};
use std::sync::{LazyLock, Mutex, MutexGuard};

use itertools::Itertools;
use rayon::iter::{IntoParallelIterator, IntoParallelRefIterator, ParallelIterator};
use serde::{Deserialize, Serialize};

use crate::build_traces::{FileTrace, LoopControl, TraceNode, TraceTree};
use crate::interner::{Interner, StringId};
use crate::raw_trace::*;

fn build_pool(jobs: usize) -> rayon::ThreadPool {
    rayon::ThreadPoolBuilder::new()
        .num_threads(jobs)
        .build()
        .expect("failed to build rayon thread pool")
}

// Pool used by the default (test-facing) `analyze` entry point
static POOL: LazyLock<rayon::ThreadPool> =
    LazyLock::new(|| build_pool(std::thread::available_parallelism().map_or(1, |n| n.get())));

#[derive(Debug, Serialize, Deserialize)]
pub struct UnitAnalysis {
    pub total_duration: i64,
    pub frontend_duration: i64,
    pub backend_duration: i64,
    pub template_instantiation_duration: i64,
    #[serde(serialize_with = "crate::report::serialize_sorted_map")]
    pub header_parse_duration: HashMap<StringId, i64>,
    // The chain is stored as the prefix of includers only (the header itself is
    // the map key); the header's own name is not repeated in its chain.
    pub header_inclusion_paths: BTreeMap<StringId, BTreeMap<Vec<StringId>, i64>>,
    #[serde(serialize_with = "crate::report::serialize_sorted_map")]
    pub function_template_instantiations: HashMap<StringId, TemplateInstantiation>,
    #[serde(serialize_with = "crate::report::serialize_sorted_map")]
    pub class_template_instantiations: HashMap<StringId, TemplateInstantiation>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

    /// Average duration per instantiation, in microseconds
    pub fn avg_duration(&self) -> i64 {
        if self.num_instantiations == 0 {
            0
        } else {
            self.total_duration / self.num_instantiations
        }
    }
}

impl UnitAnalysis {
    pub fn largest_function_template_instantiation(
        &self,
    ) -> Option<(&StringId, &TemplateInstantiation)> {
        self.function_template_instantiations
            .iter()
            .max_by(|a, b| a.1.total_duration.cmp(&b.1.total_duration))
    }

    pub fn largest_class_template_instantiation(
        &self,
    ) -> Option<(&StringId, &TemplateInstantiation)> {
        self.class_template_instantiations
            .iter()
            .max_by(|a, b| a.1.total_duration.cmp(&b.1.total_duration))
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct HeaderAnalysis {
    pub parse_duration: i64,
    pub num_inclusions: i64,
    pub inclusion_paths: BTreeMap<Vec<StringId>, i64>,
}

impl HeaderAnalysis {
    /// Average parse time per inclusion, in microseconds
    pub fn avg_inclusion_time(&self) -> i64 {
        if self.num_inclusions == 0 {
            0
        } else {
            self.parse_duration / self.num_inclusions
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct AnalysisResult {
    #[serde(serialize_with = "crate::report::serialize_sorted_map")]
    pub unit_analysis: HashMap<StringId, UnitAnalysis>,
    #[serde(serialize_with = "crate::report::serialize_sorted_map")]
    pub header_analysis: HashMap<StringId, HeaderAnalysis>,
    #[serde(serialize_with = "crate::report::serialize_sorted_map")]
    pub function_template_instantiations: HashMap<StringId, TemplateInstantiation>,
    #[serde(serialize_with = "crate::report::serialize_sorted_map")]
    pub class_template_instantiations: HashMap<StringId, TemplateInstantiation>,
}

impl AnalysisResult {
    pub fn from(unit_analysis: HashMap<StringId, UnitAnalysis>) -> Self {
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
        per_unit_analysis: &HashMap<StringId, UnitAnalysis>,
    ) -> (
        HashMap<StringId, TemplateInstantiation>,
        HashMap<StringId, TemplateInstantiation>,
    ) {
        let mut function_template_instantiations: HashMap<StringId, TemplateInstantiation> =
            HashMap::new();
        let mut class_template_instantiations: HashMap<StringId, TemplateInstantiation> =
            HashMap::new();
        for analysis in per_unit_analysis.values() {
            for (name, inst) in &analysis.function_template_instantiations {
                function_template_instantiations
                    .entry(*name)
                    .and_modify(|val| val.merge(inst))
                    .or_insert_with(|| inst.clone());
            }
            for (name, inst) in &analysis.class_template_instantiations {
                class_template_instantiations
                    .entry(*name)
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
    ) -> impl Iterator<Item = (&str, &StringId, &TemplateInstantiation)> {
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
            .k_largest_by(num_items, |a, b| {
                a.2.total_duration.cmp(&b.2.total_duration)
            })
    }

    fn get_per_header_analysis(
        per_unit_analysis: &HashMap<StringId, UnitAnalysis>,
    ) -> HashMap<StringId, HeaderAnalysis> {
        let mut header_analysis: HashMap<StringId, HeaderAnalysis> = HashMap::new();
        for analysis in per_unit_analysis.values() {
            for (header, duration) in &analysis.header_parse_duration {
                header_analysis
                    .entry(*header)
                    .and_modify(|val| {
                        val.parse_duration += *duration;
                        val.num_inclusions += 1
                    })
                    .or_insert(HeaderAnalysis {
                        parse_duration: *duration,
                        num_inclusions: 1,
                        inclusion_paths: BTreeMap::new(),
                    });
            }
        }
        for analysis in per_unit_analysis.values() {
            for (header, chain_counts) in &analysis.header_inclusion_paths {
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
    ) -> impl Iterator<Item = (&StringId, &UnitAnalysis)> {
        let num_items = min(self.unit_analysis.len(), num_items);
        self.unit_analysis.iter().k_largest_by(num_items, |a, b| {
            a.1.total_duration.cmp(&b.1.total_duration)
        })
    }

    pub fn longest_headers_parse_duration(
        &self,
        num_items: usize,
    ) -> impl Iterator<Item = (&StringId, &HeaderAnalysis)> {
        let num_items = min(self.header_analysis.len(), num_items);
        self.header_analysis.iter().k_largest_by(num_items, |a, b| {
            a.1.parse_duration.cmp(&b.1.parse_duration)
        })
    }
}

/// Returns `true` for C/C++ translation-unit files. Everything else that is
/// entered during preprocessing is a header, including extension-less standard
/// headers such as `<vector>` or `<cassert>`.
fn is_source_file(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    [
        ".c", ".cc", ".cpp", ".cxx", ".c++", ".cp", ".m", ".mm", ".s", ".asm",
    ]
    .iter()
    .any(|ext| lower.ends_with(ext))
}

fn lock(interner: &Mutex<Interner>) -> MutexGuard<'_, Interner> {
    interner
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn intern(interner: &Mutex<Interner>, s: &str) -> StringId {
    lock(interner).intern(s)
}

pub fn analyze_unit(trace: &FileTrace, interner: &Mutex<Interner>) -> (StringId, UnitAnalysis) {
    let source_file = trace.source_file.as_deref().unwrap_or(&trace.trace_file);
    let source_file_id = intern(interner, source_file);
    let mut analysis = UnitAnalysis {
        total_duration: 0,
        frontend_duration: 0,
        backend_duration: 0,
        template_instantiation_duration: 0,
        header_parse_duration: HashMap::new(),
        header_inclusion_paths: BTreeMap::new(),
        function_template_instantiations: HashMap::new(),
        class_template_instantiations: HashMap::new(),
    };
    // Extract header parse durations from the "Source" tree. Clang emits a "Source"
    // span for every entered file; every non-source file is a header, including
    // extension-less standard headers.
    if let Some(source_root_ids) = trace.trace_tree.roots.get(SOURCE) {
        let mut stack: Vec<usize> = source_root_ids.to_vec();
        while let Some(node_id) = stack.pop() {
            let node = &trace.trace_tree.nodes[node_id];
            let header_and_dur = node
                .trace
                .args
                .as_ref()
                .and_then(|args| args.detail.as_deref())
                .map(|file| file.split(':').next().unwrap_or(file))
                .filter(|file| !is_source_file(file))
                .map(|file| (file, node.trace.dur.unwrap_or(0)));
            if let Some((header, duration)) = header_and_dur {
                let header_id = intern(interner, header);
                analysis
                    .header_parse_duration
                    .entry(header_id)
                    .and_modify(|val| *val += duration)
                    .or_insert(duration);
            }
            stack.extend(node.children.iter().copied());
        }
    }
    let headers_parsed = !analysis.header_parse_duration.is_empty();
    if !headers_parsed {
        log::debug!(
            "Falling back to manual headers parsing duration for '{}'",
            trace.trace_file
        );
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
                let name_id = intern(interner, template_name);
                instantiations
                    .entry(name_id)
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
                let header = header.split(':').next().unwrap_or(header);
                if !is_source_file(header) {
                    let header_id = intern(interner, header);
                    analysis
                        .header_parse_duration
                        .entry(header_id)
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
        let mut stack: Vec<(usize, Vec<String>)> =
            source_root_ids.iter().map(|&id| (id, Vec::new())).collect();
        while let Some((node_id, mut chain)) = stack.pop() {
            let node = &trace.trace_tree.nodes[node_id];
            let file = node
                .trace
                .args
                .as_ref()
                .and_then(|args| args.detail.as_deref())
                .map(|f| f.split(':').next().unwrap_or(f).to_string());
            if let Some(file) = file {
                chain.push(file.clone());
                if !is_source_file(&file) && chain.len() > 1 {
                    // Prefix chain: everything between the top-level source file
                    // and this header, so the header's own name is not repeated.
                    let prefix = chain[1..chain.len() - 1].to_vec();
                    let header_id = intern(interner, &file);
                    let chain_ids: Vec<StringId> =
                        prefix.iter().map(|s| intern(interner, s)).collect();
                    analysis
                        .header_inclusion_paths
                        .entry(header_id)
                        .or_default()
                        .entry(chain_ids)
                        .and_modify(|count| *count += 1)
                        .or_insert(1);
                }
            }
            for &child in &node.children {
                stack.push((child, chain.clone()));
            }
        }
    }
    (source_file_id, analysis)
}

/// The result of analyzing a set of translation units: the interned analysis
/// plus the string table used to resolve its `u32` keys.
#[derive(Debug)]
pub struct Analysis {
    pub strings: Vec<String>,
    pub result: AnalysisResult,
}

impl Analysis {
    /// Resolve an interned string id to its label.
    pub fn resolve(&self, id: StringId) -> &str {
        id.resolve(&self.strings)
    }

    /// Look up the id for an interned string.
    pub fn id_of(&self, s: &str) -> Option<StringId> {
        self.strings
            .binary_search_by(|x| x.as_str().cmp(s))
            .ok()
            .map(|i| StringId::from(i as u32))
    }
}

pub fn analyze(traces: Vec<FileTrace>) -> Analysis {
    let interner = Mutex::new(Interner::default());
    let shared = &interner;
    let unit_analysis: HashMap<StringId, UnitAnalysis> = POOL.install(move || {
        traces
            .par_iter()
            .map(|trace| analyze_unit(trace, shared))
            .collect()
    });
    finalize(&interner, AnalysisResult::from(unit_analysis))
}

/// Analyze the build directory using up to `jobs` worker threads.
///
/// Each worker thread processes one trace file completely (parse -> tree ->
/// analyze) and returns only the small per-unit result, so peak memory scales
/// with the number of worker threads.
pub fn analyze_build_dir(build_dir: &str, jobs: usize, excludes: &[String]) -> Analysis {
    let paths = crate::build_traces::glob_trace_paths(build_dir, excludes);
    let interner = Mutex::new(Interner::default());
    let shared = &interner;
    let unit_analysis: HashMap<StringId, UnitAnalysis> = build_pool(jobs).install(move || {
        paths
            .into_par_iter()
            .filter_map(|path| {
                let trace = crate::build_traces::parse_trace_file(&path)?;
                let source_file = crate::build_traces::source_file_from_trace(&trace);
                let trace_tree = TraceTree::from(trace.trace_events);
                let file_trace = FileTrace {
                    trace_file: path.to_str()?.to_string(),
                    source_file: Some(source_file),
                    trace_tree,
                };
                Some(analyze_unit(&file_trace, shared))
            })
            .collect()
    });
    finalize(&interner, AnalysisResult::from(unit_analysis))
}

/// Sort the string table and remap every id so that identical inputs produce
/// identical output bytes regardless of the order strings were first interned.
fn finalize(interner: &Mutex<Interner>, result: AnalysisResult) -> Analysis {
    let strings = lock(interner).take_table();
    let mut sorted = strings.clone();
    sorted.sort();
    let new_id: HashMap<&str, StringId> = sorted
        .iter()
        .enumerate()
        .map(|(i, s)| (s.as_str(), StringId::from(i as u32)))
        .collect();
    let remap: Vec<StringId> = strings.iter().map(|s| new_id[s.as_str()]).collect();

    let remap_id = |id: StringId| remap[id.index() as usize];
    let unit_analysis = result
        .unit_analysis
        .into_iter()
        .map(|(id, mut ua)| {
            ua.header_parse_duration = ua
                .header_parse_duration
                .into_iter()
                .map(|(h, d)| (remap_id(h), d))
                .collect();
            ua.header_inclusion_paths = ua
                .header_inclusion_paths
                .into_iter()
                .map(|(h, chains)| {
                    (
                        remap_id(h),
                        chains
                            .into_iter()
                            .map(|(c, n)| (c.into_iter().map(remap_id).collect(), n))
                            .collect(),
                    )
                })
                .collect();
            ua.function_template_instantiations = ua
                .function_template_instantiations
                .into_iter()
                .map(|(n, t)| (remap_id(n), t))
                .collect();
            ua.class_template_instantiations = ua
                .class_template_instantiations
                .into_iter()
                .map(|(n, t)| (remap_id(n), t))
                .collect();
            (remap_id(id), ua)
        })
        .collect();

    let header_analysis = result
        .header_analysis
        .into_iter()
        .map(|(id, mut ha)| {
            ha.inclusion_paths = ha
                .inclusion_paths
                .into_iter()
                .map(|(c, n)| (c.into_iter().map(remap_id).collect(), n))
                .collect();
            (remap_id(id), ha)
        })
        .collect();

    let function_template_instantiations = result
        .function_template_instantiations
        .into_iter()
        .map(|(id, t)| (remap_id(id), t))
        .collect();
    let class_template_instantiations = result
        .class_template_instantiations
        .into_iter()
        .map(|(id, t)| (remap_id(id), t))
        .collect();

    Analysis {
        strings: sorted,
        result: AnalysisResult {
            unit_analysis,
            header_analysis,
            function_template_instantiations,
            class_template_instantiations,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    // Complete "X" event as emitted by clang: no category, pid == tid
    fn x_event(name: &str, pid: i64, tid: i64, ts: i64, dur: i64) -> TraceEvent {
        trace_event(name, None, "X", pid, tid, None, ts, Some(dur), None)
    }

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

    // Async "Source" begin/end events: category "Source", id 0
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

    fn file_trace(source_file: Option<&str>, events: Vec<TraceEvent>) -> FileTrace {
        FileTrace {
            trace_file: "build/main.cpp.json".to_string(),
            source_file: source_file.map(str::to_string),
            trace_tree: TraceTree::from(events),
        }
    }

    fn test_build_dir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "build_time_insight_analyzer_{label}_{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_trace_file(dir: &std::path::Path, rel_path: &str, content: &str) {
        let path = dir.join(rel_path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, content).unwrap();
    }

    fn unit_analysis(total_duration: i64) -> UnitAnalysis {
        UnitAnalysis {
            total_duration,
            frontend_duration: 0,
            backend_duration: 0,
            template_instantiation_duration: 0,
            header_parse_duration: HashMap::new(),
            header_inclusion_paths: BTreeMap::new(),
            function_template_instantiations: HashMap::new(),
            class_template_instantiations: HashMap::new(),
        }
    }

    fn header_analysis(parse_duration: i64) -> HeaderAnalysis {
        HeaderAnalysis {
            parse_duration,
            num_inclusions: 1,
            inclusion_paths: BTreeMap::new(),
        }
    }

    fn analysis_result(
        function_instantiations: &[(&str, i64)],
        class_instantiations: &[(&str, i64)],
    ) -> (AnalysisResult, Interner) {
        let mut interner = Interner::default();
        let mut result = AnalysisResult {
            unit_analysis: HashMap::new(),
            header_analysis: HashMap::new(),
            function_template_instantiations: HashMap::new(),
            class_template_instantiations: HashMap::new(),
        };
        for (name, dur) in function_instantiations {
            result
                .function_template_instantiations
                .insert(interner.intern(name), TemplateInstantiation::new(*dur));
        }
        for (name, dur) in class_instantiations {
            result
                .class_template_instantiations
                .insert(interner.intern(name), TemplateInstantiation::new(*dur));
        }
        (result, interner)
    }

    fn resolve(interner: &Mutex<Interner>, id: StringId) -> String {
        interner.lock().unwrap().resolve(id).to_string()
    }

    fn analyze_unit_with(trace: FileTrace) -> (Mutex<Interner>, StringId, UnitAnalysis) {
        let interner = Mutex::new(Interner::default());
        let (id, unit) = analyze_unit(&trace, &interner);
        (interner, id, unit)
    }

    #[test]
    fn template_instantiation_new_and_add() {
        let mut inst = TemplateInstantiation::new(10);
        assert_eq!(inst.total_duration, 10);
        assert_eq!(inst.num_instantiations, 1);
        inst.add(15);
        assert_eq!(inst.total_duration, 25);
        assert_eq!(inst.num_instantiations, 2);
    }

    #[test]
    fn template_instantiation_merge() {
        let mut inst = TemplateInstantiation::new(10);
        inst.merge(&TemplateInstantiation::new(20));
        assert_eq!(inst.total_duration, 30);
        assert_eq!(inst.num_instantiations, 2);
    }

    #[test]
    fn template_instantiation_average_duration() {
        let mut inst = TemplateInstantiation::new(30);
        inst.add(50);
        assert_eq!(inst.total_duration, 80);
        assert_eq!(inst.num_instantiations, 2);
        assert_eq!(inst.avg_duration(), 40);
    }

    #[test]
    fn template_instantiation_average_is_zero_when_empty() {
        let inst = TemplateInstantiation {
            total_duration: 0,
            num_instantiations: 0,
        };
        assert_eq!(inst.avg_duration(), 0);
    }

    #[test]
    fn header_analysis_average_inclusion_time() {
        let header = HeaderAnalysis {
            parse_duration: 300,
            num_inclusions: 3,
            inclusion_paths: BTreeMap::new(),
        };
        assert_eq!(header.avg_inclusion_time(), 100);
    }

    #[test]
    fn largest_template_instantiation_returns_max_by_duration() {
        let mut interner = Interner::default();
        let a = interner.intern("a");
        let b = interner.intern("b");
        let c = interner.intern("c");
        let mut unit = unit_analysis(0);
        unit.function_template_instantiations
            .insert(a, TemplateInstantiation::new(10));
        unit.function_template_instantiations
            .insert(b, TemplateInstantiation::new(30));
        unit.class_template_instantiations
            .insert(c, TemplateInstantiation::new(20));

        let (name, inst) = unit.largest_function_template_instantiation().unwrap();
        assert_eq!(interner.resolve(*name), "b");
        assert_eq!(inst.total_duration, 30);

        let (name, inst) = unit.largest_class_template_instantiation().unwrap();
        assert_eq!(interner.resolve(*name), "c");
        assert_eq!(inst.total_duration, 20);
    }

    #[test]
    fn largest_template_instantiation_is_none_when_empty() {
        assert!(
            unit_analysis(0)
                .largest_function_template_instantiation()
                .is_none()
        );
        assert!(
            unit_analysis(0)
                .largest_class_template_instantiation()
                .is_none()
        );
    }

    #[test]
    fn analysis_result_merges_template_instantiations_across_units() {
        let mut interner = Interner::default();
        let vector = interner.intern("std::vector<int>");
        let allocator = interner.intern("std::allocator");
        let map = interner.intern("std::map");
        let a_cpp = interner.intern("a.cpp");
        let b_cpp = interner.intern("b.cpp");

        let mut a = unit_analysis(100);
        a.function_template_instantiations
            .insert(vector, TemplateInstantiation::new(100));
        a.function_template_instantiations
            .insert(allocator, TemplateInstantiation::new(50));
        a.class_template_instantiations
            .insert(map, TemplateInstantiation::new(70));

        let mut b = unit_analysis(200);
        b.function_template_instantiations
            .insert(vector, TemplateInstantiation::new(200));
        b.class_template_instantiations
            .insert(map, TemplateInstantiation::new(30));

        let result = AnalysisResult::from(HashMap::from([(a_cpp, a), (b_cpp, b)]));

        let vector_inst = &result.function_template_instantiations[&vector];
        assert_eq!(vector_inst.total_duration, 300);
        assert_eq!(vector_inst.num_instantiations, 2);
        assert_eq!(
            result.function_template_instantiations[&allocator].total_duration,
            50
        );
        let map_inst = &result.class_template_instantiations[&map];
        assert_eq!(map_inst.total_duration, 100);
        assert_eq!(map_inst.num_instantiations, 2);
    }

    #[test]
    fn analysis_result_aggregates_header_parse_durations() {
        let mut interner = Interner::default();
        let a_h = interner.intern("a.h");
        let b_h = interner.intern("b.h");
        let c_h = interner.intern("c.h");
        let a_cpp = interner.intern("a.cpp");
        let b_cpp = interner.intern("b.cpp");

        let mut a = unit_analysis(0);
        a.header_parse_duration = HashMap::from([(a_h, 100), (b_h, 50)]);
        a.header_inclusion_paths
            .insert(b_h, BTreeMap::from([(vec![a_h], 1)]));
        let mut b = unit_analysis(0);
        b.header_parse_duration = HashMap::from([(a_h, 200), (c_h, 30)]);
        b.header_inclusion_paths
            .insert(b_h, BTreeMap::from([(vec![a_h], 2)]));

        let result = AnalysisResult::from(HashMap::from([(a_cpp, a), (b_cpp, b)]));

        let a_h_analysis = &result.header_analysis[&a_h];
        assert_eq!(a_h_analysis.parse_duration, 300);
        assert_eq!(a_h_analysis.num_inclusions, 2);

        let b_h_analysis = &result.header_analysis[&b_h];
        assert_eq!(b_h_analysis.parse_duration, 50);
        assert_eq!(b_h_analysis.num_inclusions, 1);
        assert_eq!(b_h_analysis.inclusion_paths[&vec![a_h]], 3);

        let c_h_analysis = &result.header_analysis[&c_h];
        assert_eq!(c_h_analysis.parse_duration, 30);
        assert_eq!(c_h_analysis.num_inclusions, 1);
    }

    #[test]
    fn longest_template_instantiations_returns_top_k() {
        let (result, interner) = analysis_result(
            &[("f1", 10), ("f2", 50), ("f3", 30)],
            &[("c1", 20), ("c2", 70)],
        );
        let mut top: Vec<_> = result.longest_template_instantiations(2).collect();
        top.sort_by_key(|x| std::cmp::Reverse(x.2.total_duration));
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].0, "class");
        assert_eq!(interner.resolve(*top[0].1), "c2");
        assert_eq!(top[0].2.total_duration, 70);
        assert_eq!(top[1].0, "function");
        assert_eq!(interner.resolve(*top[1].1), "f2");
        assert_eq!(top[1].2.total_duration, 50);
    }

    #[test]
    fn longest_unit_compilations_returns_top_k() {
        let mut interner = Interner::default();
        let a = interner.intern("a.cpp");
        let b = interner.intern("b.cpp");
        let c = interner.intern("c.cpp");
        let result = AnalysisResult {
            unit_analysis: HashMap::from([
                (a, unit_analysis(50)),
                (b, unit_analysis(200)),
                (c, unit_analysis(100)),
            ]),
            header_analysis: HashMap::new(),
            function_template_instantiations: HashMap::new(),
            class_template_instantiations: HashMap::new(),
        };
        let mut top: Vec<_> = result.longest_unit_compilations(2).collect();
        top.sort_by_key(|x| std::cmp::Reverse(x.1.total_duration));
        assert_eq!(top.len(), 2);
        assert_eq!(interner.resolve(*top[0].0), "b.cpp");
        assert_eq!(top[0].1.total_duration, 200);
        assert_eq!(interner.resolve(*top[1].0), "c.cpp");
        assert_eq!(top[1].1.total_duration, 100);
    }

    #[test]
    fn longest_headers_parse_duration_returns_top_k() {
        let mut interner = Interner::default();
        let a = interner.intern("a.h");
        let b = interner.intern("b.h");
        let c = interner.intern("c.h");
        let result = AnalysisResult {
            unit_analysis: HashMap::new(),
            header_analysis: HashMap::from([
                (a, header_analysis(10)),
                (b, header_analysis(90)),
                (c, header_analysis(50)),
            ]),
            function_template_instantiations: HashMap::new(),
            class_template_instantiations: HashMap::new(),
        };
        let mut top: Vec<_> = result.longest_headers_parse_duration(2).collect();
        top.sort_by_key(|x| std::cmp::Reverse(x.1.parse_duration));
        assert_eq!(top.len(), 2);
        assert_eq!(interner.resolve(*top[0].0), "b.h");
        assert_eq!(interner.resolve(*top[1].0), "c.h");
    }

    #[test]
    fn analyze_extracts_total_frontend_and_backend_durations() {
        let trace = file_trace(
            Some("src/main.cpp"),
            vec![
                x_event("ExecuteCompiler", 1, 1, 0, 1000),
                x_event("Frontend", 1, 1, 10, 400),
                x_event("Backend", 1, 1, 500, 400),
                x_event("Total ExecuteCompiler", 1, 2, 0, 1000),
                x_event("Total Frontend", 1, 3, 0, 400),
                x_event("Total Backend", 1, 4, 0, 400),
            ],
        );
        let (interner, unit_id, unit) = analyze_unit_with(trace);
        assert_eq!(resolve(&interner, unit_id), "src/main.cpp");
        assert_eq!(unit.total_duration, 1000);
        assert_eq!(unit.frontend_duration, 400);
        assert_eq!(unit.backend_duration, 400);
    }

    #[test]
    fn analyze_counts_template_instantiations_and_skips_nested() {
        let trace = file_trace(
            Some("src/main.cpp"),
            vec![
                x_event("Backend", 1, 1, 500, 400),
                x_event_detail(
                    "InstantiateFunction",
                    1,
                    1,
                    510,
                    150,
                    "std::vector<int>::push_back",
                ),
                x_event_detail(
                    "InstantiateFunction",
                    1,
                    1,
                    515,
                    40,
                    "std::allocator<int>::allocate",
                ),
                x_event_detail("InstantiateClass", 1, 1, 670, 80, "std::vector<int>"),
                x_event("PerformPendingInstantiations", 1, 1, 760, 100),
            ],
        );
        let (interner, _, unit) = analyze_unit_with(trace);
        // 150 (top-level) + 80 (class) + 100 (pending); the nested allocate is excluded
        assert_eq!(unit.template_instantiation_duration, 330);
        let push_back = &unit.function_template_instantiations
            [&intern(&interner, "std::vector<int>::push_back")];
        assert_eq!(push_back.total_duration, 150);
        assert_eq!(push_back.num_instantiations, 1);
        let allocate = &unit.function_template_instantiations
            [&intern(&interner, "std::allocator<int>::allocate")];
        assert_eq!(allocate.total_duration, 40);
        assert_eq!(allocate.num_instantiations, 1);
        let vec = &unit.class_template_instantiations[&intern(&interner, "std::vector<int>")];
        assert_eq!(vec.total_duration, 80);
        assert_eq!(vec.num_instantiations, 1);
    }

    #[test]
    fn analyze_extracts_header_parse_duration_from_source_spans() {
        let trace = file_trace(
            Some("src/main.cpp"),
            vec![
                x_event("ExecuteCompiler", 1, 1, 0, 1000),
                source_begin(1, 1, 100, "src/app.h:5:1"),
                source_end(1, 1, 400),
            ],
        );
        let (interner, _, unit) = analyze_unit_with(trace);
        assert_eq!(
            unit.header_parse_duration[&intern(&interner, "src/app.h")],
            300
        );
    }

    #[test]
    fn analyze_counts_nested_and_extensionless_headers() {
        let trace = file_trace(
            Some("src/main.cpp"),
            vec![
                x_event("ExecuteCompiler", 1, 1, 0, 1000),
                source_begin(1, 1, 100, "src/main.cpp"),
                source_end(1, 1, 900),
                source_begin(1, 1, 120, "cassert"),
                source_end(1, 1, 800),
                source_begin(1, 1, 130, "/usr/include/assert.h:5:1"),
                source_end(1, 1, 700),
            ],
        );
        let (interner, _, unit) = analyze_unit_with(trace);
        // The .cpp translation unit is skipped; extension-less and nested headers count
        assert!(
            !unit
                .header_parse_duration
                .contains_key(&intern(&interner, "src/main.cpp"))
        );
        assert_eq!(
            unit.header_parse_duration[&intern(&interner, "cassert")],
            680
        );
        assert_eq!(
            unit.header_parse_duration[&intern(&interner, "/usr/include/assert.h")],
            570
        );
    }

    #[test]
    fn analyze_falls_back_to_parse_events_for_headers() {
        let trace = file_trace(
            Some("src/main.cpp"),
            vec![
                x_event("Frontend", 1, 1, 0, 500),
                x_event_detail(
                    "ParseDeclarationOrFunctionDefinition",
                    1,
                    1,
                    10,
                    100,
                    "lib/foo.h:3:5",
                ),
                x_event_detail(
                    "ParseDeclarationOrFunctionDefinition",
                    1,
                    1,
                    20,
                    60,
                    "lib/foo.h:12:1",
                ),
                x_event_detail(
                    "ParseDeclarationOrFunctionDefinition",
                    1,
                    1,
                    30,
                    40,
                    "src/main.cpp:5:1",
                ),
            ],
        );
        let (interner, _, unit) = analyze_unit_with(trace);
        assert_eq!(
            unit.header_parse_duration[&intern(&interner, "lib/foo.h")],
            160
        );
    }

    #[test]
    fn analyze_collects_header_inclusion_paths() {
        let trace = file_trace(
            Some("src/main.cpp"),
            vec![
                source_begin(1, 1, 100, "src/main.cpp"),
                source_end(1, 1, 300),
                source_begin(1, 1, 120, "lib/a.h:10:2"),
                source_end(1, 1, 250),
                source_begin(1, 1, 130, "lib/b.h:5:1"),
                source_end(1, 1, 240),
            ],
        );
        let (interner, _, unit) = analyze_unit_with(trace);
        let a_h = intern(&interner, "lib/a.h");
        let b_h = intern(&interner, "lib/b.h");
        // a.h is included directly from the source file, so its chain is empty
        assert_eq!(
            unit.header_inclusion_paths[&a_h][&Vec::<StringId>::new()],
            1
        );
        // b.h is included via a.h, so its chain stores the prefix [a.h]
        assert_eq!(unit.header_inclusion_paths[&b_h][&vec![a_h]], 1);
    }

    #[test]
    fn analyze_uses_trace_file_when_source_file_missing() {
        let trace = file_trace(None, vec![x_event("Total ExecuteCompiler", 1, 2, 0, 100)]);
        let result = analyze(vec![trace]);
        let unit_id = result.id_of("build/main.cpp.json").unwrap();
        let unit = &result.result.unit_analysis[&unit_id];
        assert_eq!(unit.total_duration, 100);
    }

    #[test]
    fn analyze_build_dir_parses_and_analyzes_trace_files() {
        let dir = test_build_dir("analyze_build_dir");
        let valid = r#"{
            "traceEvents": [
                { "pid": 7, "tid": 7, "ts": 0, "ph": "X", "dur": 1000, "name": "OptModule", "args": { "detail": "src/main.cpp" } },
                { "pid": 7, "tid": 7, "ts": 0, "ph": "X", "dur": 100, "name": "Total ExecuteCompiler" }
            ],
            "beginningOfTime": 1785441886055364
        }"#;
        write_trace_file(&dir, "main.cpp.json", valid);
        write_trace_file(&dir, "broken.cpp.json", "{ not valid json");
        write_trace_file(&dir, "ignored.json", valid);

        let result = analyze_build_dir(dir.to_str().unwrap(), 2, &[]);
        std::fs::remove_dir_all(&dir).ok();

        assert_eq!(result.result.unit_analysis.len(), 1);
        let unit_id = result.id_of("src/main.cpp").unwrap();
        let unit = &result.result.unit_analysis[&unit_id];
        assert_eq!(unit.total_duration, 100);
    }
}

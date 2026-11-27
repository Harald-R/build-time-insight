//! Builds the CMake target dependency graph.
//!
//! The graph comes from the CMake File API (`codemodel-v2`) when a reply
//! exists. If it does not, the code reads the Ninja build file. Each target
//! gets the summed compile time of its translation units.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One CMake target.
#[derive(Debug, Serialize, Deserialize)]
pub struct TargetNode {
    /// Target name, for example `geometry_lib`.
    pub name: String,
    /// Artifact kind: `executable`, `static`, `shared`, `object`, `interface`,
    /// or `unknown`.
    pub kind: String,
    /// Most common source directory of the target. Empty when unknown.
    #[serde(default)]
    pub dir: String,
    /// Summed compile time of the target's translation units, in microseconds.
    pub duration: i64,
    /// Number of translation units in the target.
    pub source_count: usize,
}

/// One link dependency between two targets.
#[derive(Debug, Serialize, Deserialize)]
pub struct TargetEdge {
    /// Name of the dependent target.
    pub from: String,
    /// Name of the dependency target.
    pub to: String,
}

/// The CMake target dependency graph.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct TargetGraph {
    /// All targets.
    pub targets: Vec<TargetNode>,
    /// All link dependencies.
    pub edges: Vec<TargetEdge>,
}

impl TargetGraph {
    /// Returns true when the graph holds no target.
    pub fn is_empty(&self) -> bool {
        self.targets.is_empty()
    }
}

/// A target before duration attribution: name, kind, and dependencies.
struct RawTarget {
    name: String,
    kind: String,
    deps: Vec<String>,
}

/// Builds the target graph for `build_dir`. Maps each target to the summed
/// compile time of its units. `durations` maps an absolute source path to the
/// unit's total duration. Returns `None` when no target data is found.
pub fn build(
    build_dir: &str,
    root: Option<&str>,
    durations: &HashMap<String, i64>,
) -> Option<TargetGraph> {
    let raw = load_raw_targets(build_dir)?;
    if raw.is_empty() {
        return None;
    }

    // Target per source, from the compile commands.
    let source_target = load_source_targets(build_dir, root);

    // Sum the durations per target.
    let mut per_target: HashMap<String, (i64, usize)> = HashMap::new();
    for (source, duration) in durations {
        if let Some(target) = source_target.get(source) {
            let entry = per_target.entry(target.clone()).or_insert((0, 0));
            entry.0 += *duration;
            entry.1 += 1;
        }
    }

    // The most common source directory of each target, relative to the root.
    let mut dir_counts: HashMap<String, HashMap<String, usize>> = HashMap::new();
    for (source, target) in &source_target {
        let dir = source
            .rsplit_once('/')
            .map(|(d, _)| d)
            .unwrap_or("")
            .to_string();
        *dir_counts
            .entry(target.clone())
            .or_default()
            .entry(dir)
            .or_insert(0) += 1;
    }
    let target_dir = |target: &str| -> String {
        let dir = dir_counts
            .get(target)
            .and_then(|counts| counts.iter().max_by_key(|(_, count)| **count))
            .map(|(dir, _)| dir.clone())
            .unwrap_or_default();
        match root {
            Some(root) => dir
                .strip_prefix(root)
                .and_then(|rest| rest.strip_prefix('/'))
                .map(str::to_string)
                .unwrap_or(dir),
            None => dir,
        }
    };

    let names: Vec<String> = raw.iter().map(|t| t.name.clone()).collect();
    let name_set: std::collections::HashSet<&str> = names.iter().map(|s| s.as_str()).collect();

    let targets = raw
        .iter()
        .map(|t| {
            let (duration, source_count) = per_target.get(&t.name).copied().unwrap_or((0, 0));
            TargetNode {
                name: t.name.clone(),
                kind: t.kind.clone(),
                dir: target_dir(&t.name),
                duration,
                source_count,
            }
        })
        .collect();

    let mut edges = Vec::new();
    for t in &raw {
        for dep in &t.deps {
            // Only keep edges between known targets.
            if name_set.contains(dep.as_str()) {
                edges.push(TargetEdge {
                    from: t.name.clone(),
                    to: dep.clone(),
                });
            }
        }
    }

    Some(TargetGraph { targets, edges })
}

/// Loads the raw targets from the CMake File API, then from the Ninja file.
fn load_raw_targets(build_dir: &str) -> Option<Vec<RawTarget>> {
    if let Some(targets) = load_codemodel(build_dir)
        && !targets.is_empty()
    {
        return Some(targets);
    }
    load_ninja(build_dir)
}

// ---------------------------------------------------------------------------
// CMake File API (codemodel-v2)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct ApiIndex {
    #[serde(default)]
    objects: Vec<ApiObject>,
}

#[derive(Deserialize)]
struct ApiObject {
    #[serde(default)]
    kind: String,
    #[serde(default, rename = "jsonFile")]
    json_file: String,
}

#[derive(Deserialize)]
struct Codemodel {
    #[serde(default)]
    configurations: Vec<Configuration>,
}

#[derive(Deserialize)]
struct Configuration {
    #[serde(default)]
    targets: Vec<CodemodelTarget>,
}

#[derive(Deserialize)]
struct CodemodelTarget {
    #[serde(default)]
    name: String,
    #[serde(default)]
    id: String,
    #[serde(default, rename = "jsonFile")]
    json_file: String,
}

#[derive(Deserialize)]
struct TargetDetail {
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    dependencies: Vec<Dependency>,
}

#[derive(Deserialize)]
struct Dependency {
    #[serde(default)]
    id: String,
}

/// Reads the codemodel reply. Returns the targets with their dependencies.
fn load_codemodel(build_dir: &str) -> Option<Vec<RawTarget>> {
    let reply_dir = Path::new(build_dir).join(".cmake/api/v1/reply");
    let index_path = find_file(&reply_dir, "index-")?;
    let index: ApiIndex = read_json(&index_path)?;

    let codemodel_file = index
        .objects
        .iter()
        .find(|o| o.kind == "codemodel")?
        .json_file
        .clone();
    let codemodel: Codemodel = read_json(&reply_dir.join(codemodel_file))?;

    // Collect the target entries. Map each id to its name.
    let mut entries: Vec<CodemodelTarget> = Vec::new();
    let mut id_to_name: BTreeMap<String, String> = BTreeMap::new();
    for config in &codemodel.configurations {
        for target in &config.targets {
            if !target.id.is_empty() {
                id_to_name.insert(target.id.clone(), target.name.clone());
            }
            entries.push(CodemodelTarget {
                name: target.name.clone(),
                id: target.id.clone(),
                json_file: target.json_file.clone(),
            });
        }
    }

    let mut seen = std::collections::HashSet::new();
    let mut result = Vec::new();
    for entry in &entries {
        if entry.name.is_empty() || !seen.insert(entry.id.clone()) {
            continue;
        }
        // The kind and the dependencies are in the target detail file.
        let detail: Option<TargetDetail> = read_json(&reply_dir.join(&entry.json_file));
        let kind = detail
            .as_ref()
            .map(|d| normalize_kind(&d.kind))
            .unwrap_or_else(|| "unknown".to_string());
        let deps = detail
            .map(|d| {
                d.dependencies
                    .iter()
                    .filter_map(|dep| id_to_name.get(&dep.id).cloned())
                    .collect()
            })
            .unwrap_or_default();
        result.push(RawTarget {
            name: entry.name.clone(),
            kind,
            deps,
        });
    }
    Some(result)
}

/// Maps a CMake target type to a short kind.
fn normalize_kind(kind: &str) -> String {
    match kind {
        "EXECUTABLE" => "executable",
        "STATIC_LIBRARY" => "static",
        "SHARED_LIBRARY" | "MODULE_LIBRARY" => "shared",
        "OBJECT_LIBRARY" => "object",
        "INTERFACE_LIBRARY" => "interface",
        _ => "unknown",
    }
    .to_string()
}

/// Finds the first file in `dir` whose name starts with `prefix`.
fn find_file(dir: &Path, prefix: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut found: Option<PathBuf> = None;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with(prefix) {
            found = Some(entry.path());
        }
    }
    found
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let data = std::fs::read(path).ok()?;
    serde_json::from_slice(&data).ok()
}

// ---------------------------------------------------------------------------
// Ninja fallback
// ---------------------------------------------------------------------------

/// Reads the target graph from `build.ninja` link edges. A link edge names the
/// target and its inputs. A library input becomes a dependency edge.
fn load_ninja(build_dir: &str) -> Option<Vec<RawTarget>> {
    let path = Path::new(build_dir).join("build.ninja");
    let text = std::fs::read_to_string(&path).ok()?;

    // Target name -> (kind, deps).
    let mut targets: BTreeMap<String, (String, Vec<String>)> = BTreeMap::new();

    for line in text.lines() {
        let line = line.trim_end();
        let Some(rest) = line.strip_prefix("build ") else {
            continue;
        };
        // Form: build <outputs>: <rule> <inputs> [| <order-only inputs>]
        let Some((outputs, rhs)) = rest.split_once(": ") else {
            continue;
        };
        let output = outputs.split_whitespace().next().unwrap_or("");
        // Only link rules define targets.
        let rule = rhs.split_whitespace().next().unwrap_or("");
        let kind = if rule.contains("STATIC_LIBRARY_LINKER") {
            "static"
        } else if rule.contains("SHARED_LIBRARY_LINKER") || rule.contains("MODULE_LIBRARY_LINKER") {
            "shared"
        } else if rule.contains("EXECUTABLE_LINKER") {
            "executable"
        } else {
            continue;
        };

        let name = target_name_from_artifact(output);
        if name.is_empty() {
            continue;
        }

        // Inputs are the tokens after the rule. The `|` separator also starts
        // dependency inputs, so treat every token that names a library as a
        // dependency.
        let deps: Vec<String> = rhs
            .split_whitespace()
            .skip(1)
            .filter_map(|input| {
                let base = input.rsplit('/').next().unwrap_or(input);
                if is_library(base) {
                    let dep = target_name_from_artifact(base);
                    (!dep.is_empty() && dep != name).then_some(dep)
                } else {
                    None
                }
            })
            .collect();

        targets
            .entry(name)
            .and_modify(|e| e.1.extend(deps.clone()))
            .or_insert((kind.to_string(), deps));
    }

    if targets.is_empty() {
        return None;
    }
    Some(
        targets
            .into_iter()
            .map(|(name, (kind, mut deps))| {
                deps.sort();
                deps.dedup();
                RawTarget { name, kind, deps }
            })
            .collect(),
    )
}

/// Returns the target name for an artifact path.
fn target_name_from_artifact(path: &str) -> String {
    let base = path.rsplit('/').next().unwrap_or(path);
    let mut name = base;
    if let Some(rest) = name.strip_prefix("lib") {
        name = rest;
    }
    // Strip the extension: `.a`, `.so`, `.so.1`, `.dylib`, and object files.
    for sep in [".so", ".a", ".dylib", ".lib"] {
        if let Some(i) = name.find(sep) {
            name = &name[..i];
        }
    }
    if let Some(i) = name.find(".o") {
        name = &name[..i];
    }
    name.to_string()
}

/// Returns true when the file name is a static or shared library.
fn is_library(base: &str) -> bool {
    (base.starts_with("lib") && (base.contains(".a") || base.contains(".so")))
        || base.ends_with(".lib")
}

// ---------------------------------------------------------------------------
// Source -> target attribution
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct CompileCommand {
    #[serde(default)]
    directory: String,
    #[serde(default)]
    file: String,
    #[serde(default)]
    command: String,
    #[serde(default)]
    arguments: Option<Vec<String>>,
}

/// Resolves the source path from a compile command. Committed fixture databases
/// use project-root-relative paths; CMake databases use absolute paths.
fn compile_command_source(directory: &str, file: &str, root: Option<&str>) -> String {
    if Path::new(file).is_absolute() {
        return file.to_string();
    }
    if Path::new(directory).is_absolute() {
        return format!("{}/{}", directory.trim_end_matches('/'), file);
    }
    match root {
        Some(root) => format!("{}/{}", root.trim_end_matches('/'), file),
        None => file.to_string(),
    }
}

/// Maps each absolute source path to its target, from the compile commands.
/// The target comes from the object path `CMakeFiles/<target>.dir/...`.
fn load_source_targets(build_dir: &str, root: Option<&str>) -> HashMap<String, String> {
    let path = Path::new(build_dir).join("compile_commands.json");
    let Some(commands) = read_json::<Vec<CompileCommand>>(&path) else {
        return HashMap::new();
    };
    let mut map = HashMap::new();
    for cmd in commands {
        let args = match cmd.arguments {
            Some(args) => args,
            None => cmd.command.split_whitespace().map(str::to_string).collect(),
        };
        if let Some(output) = find_output(&args)
            && let Some(target) = target_from_object_path(&output)
        {
            map.insert(
                compile_command_source(&cmd.directory, &cmd.file, root),
                target,
            );
        }
    }
    map
}

/// Finds the `-o` output path in a compile command.
fn find_output(args: &[String]) -> Option<String> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == "-o" {
            return iter.next().cloned();
        }
        if let Some(rest) = arg.strip_prefix("-o")
            && !rest.is_empty()
        {
            return Some(rest.to_string());
        }
    }
    None
}

/// Extracts the target name from an object path such as
/// `CMakeFiles/geometry_lib.dir/src/geometry.cpp.o`.
fn target_from_object_path(path: &str) -> Option<String> {
    let start = path.find("CMakeFiles/")? + "CMakeFiles/".len();
    let rest = &path[start..];
    let end = rest.find(".dir")?;
    Some(rest[..end].to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_target_name_from_artifacts() {
        assert_eq!(
            target_name_from_artifact("libgeometry_lib.a"),
            "geometry_lib"
        );
        assert_eq!(target_name_from_artifact("libfoo.so.1"), "foo");
        assert_eq!(target_name_from_artifact("shapely"), "shapely");
        assert_eq!(
            target_name_from_artifact("CMakeFiles/shapely.dir/src/main.cpp.o"),
            "main.cpp"
        );
    }

    #[test]
    fn recognizes_libraries() {
        assert!(is_library("libfoo.a"));
        assert!(is_library("libbar.so"));
        assert!(!is_library("main.o"));
        assert!(!is_library("shapely"));
    }

    #[test]
    fn extracts_target_from_object_path() {
        assert_eq!(
            target_from_object_path("CMakeFiles/geometry_lib.dir/src/geometry.cpp.o").as_deref(),
            Some("geometry_lib")
        );
        assert_eq!(target_from_object_path("src/main.cpp.o"), None);
    }

    #[test]
    fn resolves_relative_compile_command_sources() {
        assert_eq!(
            compile_command_source(
                "tests/data/cpp-general-project/build-clang-18",
                "tests/data/cpp-general-project/src/a.cpp",
                Some("/repo"),
            ),
            "/repo/tests/data/cpp-general-project/src/a.cpp"
        );
        assert_eq!(
            compile_command_source("/build", "src/a.cpp", None),
            "/build/src/a.cpp"
        );
        assert_eq!(
            compile_command_source("", "/abs/src/a.cpp", Some("/repo")),
            "/abs/src/a.cpp"
        );
    }

    #[test]
    fn finds_output_flag() {
        let args: Vec<String> = ["clang++", "-o", "CMakeFiles/a.dir/x.o", "-c", "x.cpp"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(find_output(&args).as_deref(), Some("CMakeFiles/a.dir/x.o"));
    }
}

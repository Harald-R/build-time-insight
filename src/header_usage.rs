use std::fs;
use std::io::{BufReader, Cursor, Read};
use std::path::Path;
use std::process::{Child, Stdio};

use serde::de::{
    self, DeserializeSeed, Deserializer as _, IgnoredAny, MapAccess, SeqAccess, Visitor,
};
use serde::{Deserialize, Serialize};

/// How a translation unit uses a header it includes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HeaderUsage {
    /// No symbol from the header is referenced; the `#include` can be removed.
    Unused,
    /// Only declarations are used; the include can be replaced by a forward
    /// declaration.
    DeclarationOnly,
    /// A definition-requiring use exists (construction / instantiation).
    NeedsDefinition,
}

/// Run clang on a single translation unit and return, for each included header,
/// how the unit uses it. `headers` must be in the analysis's stored form
/// (relative to the project root for project files, absolute for others).
pub fn analyze_unit_header_usage(
    source: &str,
    root: Option<&str>,
    headers: &[String],
    compile_commands: &Option<CompileCommands>,
) -> Vec<(String, HeaderUsage)> {
    let Some((mut child, summary)) = dump_ast(source, root, headers, compile_commands) else {
        return Vec::new();
    };
    let success = child.wait().ok().is_some_and(|s| s.success());
    if !success {
        return Vec::new();
    }
    classify(&summary, source, headers)
}

/// Compile commands for the build, loaded from `compile_commands.json` in the
/// build directory (or its nearest ancestor that has one).
pub struct CompileCommands {
    entries: Vec<CompileCommand>,
}

impl CompileCommands {
    pub fn load(build_dir: &str) -> Option<CompileCommands> {
        let mut dir = Path::new(build_dir);
        loop {
            let path = dir.join("compile_commands.json");
            if path.is_file() {
                let content = fs::read_to_string(&path).ok()?;
                let entries: Vec<CompileCommand> = serde_json::from_str(&content).ok()?;
                return Some(CompileCommands { entries });
            }
            dir = dir.parent()?;
        }
    }

    fn for_source(&self, absolute_source: &str) -> Option<&CompileCommand> {
        self.entries
            .iter()
            .find(|entry| entry.file == absolute_source)
    }
}

#[derive(Deserialize)]
struct CompileCommand {
    directory: String,
    file: String,
    #[serde(default)]
    arguments: Option<Vec<String>>,
    #[serde(default)]
    command: Option<String>,
}

/// Spawn clang and stream its `-ast-dump=json` stdout into a compact `Summary`.
/// The (potentially hundreds of MB) dump is consumed incrementally and never
/// materialized in memory. Returns the child process (to check its exit status)
/// together with the extracted summary.
fn dump_ast(
    source: &str,
    root: Option<&str>,
    headers: &[String],
    compile_commands: &Option<CompileCommands>,
) -> Option<(Child, Summary)> {
    let absolute_source = match root {
        Some(r) => format!("{}/{}", r, source),
        None => source.to_string(),
    };

    let (directory, flags) = if let Some(entry) = compile_commands
        .as_ref()
        .and_then(|cc| cc.for_source(&absolute_source))
    {
        // Use the recorded compile flags, dropping output/dependency options and
        // the source file itself.
        let argv = entry
            .arguments
            .clone()
            .unwrap_or_else(|| shell_split(entry.command.as_deref().unwrap_or("")));
        let mut flags = Vec::new();
        let mut skip_next = false;
        for arg in argv.into_iter().skip(1) {
            if skip_next {
                skip_next = false;
                continue;
            }
            match arg.as_str() {
                "-c" | "-MD" | "-MMD" | "-MP" | "-MG" | "-MM" => continue,
                "-o" | "-MF" | "-MT" | "-MQ" | "-MJ" => {
                    skip_next = true;
                    continue;
                }
                _ if arg.starts_with("-o")
                    || arg.starts_with("-MF")
                    || arg.starts_with("-MT")
                    || arg.starts_with("-MJ")
                    || arg.ends_with(".c")
                    || arg.ends_with(".cc")
                    || arg.ends_with(".cpp")
                    || arg.ends_with(".cxx")
                    || arg.ends_with(".m")
                    || arg.ends_with(".mm")
                    || arg.ends_with(".S") =>
                {
                    continue;
                }
                _ => flags.push(arg),
            }
        }
        flags.push("-fsyntax-only".into());
        flags.push("-Xclang".into());
        flags.push("-ast-dump=json".into());
        flags.push(absolute_source);
        (entry.directory.clone(), flags)
    } else {
        // Fallback: best-effort compile with no extra flags.
        let flags: Vec<String> = vec![
            "-std=c++17".into(),
            "-fsyntax-only".into(),
            "-Xclang".into(),
            "-ast-dump=json".into(),
            source.into(),
        ];
        (root.unwrap_or("").to_string(), flags)
    };

    let mut cmd = std::process::Command::new("clang++");
    cmd.current_dir(&directory);
    cmd.args(&flags);
    cmd.stdout(Stdio::piped()).stderr(Stdio::null());
    let mut child = cmd.spawn().ok()?;
    let stdout = child.stdout.take()?;
    let mut reader = BufReader::new(stdout);

    // Skip any diagnostics before the JSON document (which starts at the first
    // '{'); on a clean compile there are none.
    let mut prefix = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        if reader.read_exact(&mut byte).is_err() {
            let _ = child.kill();
            return None;
        }
        prefix.push(byte[0]);
        if byte[0] == b'{' {
            break;
        }
        if prefix.len() > 4096 {
            let _ = child.kill();
            return None;
        }
    }
    let stream = Cursor::new(prefix).chain(reader);
    let mut de = serde_json::Deserializer::from_reader(stream);
    let mut summary = Summary::default();
    let _ = de.deserialize_any(NodeVisitor {
        summary: &mut summary,
        root,
        source,
        headers,
        inherited: None,
    });
    Some((child, summary))
}

/// Minimal POSIX-ish shell tokenizer for splitting a `command` string.
fn shell_split(s: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut single = false;
    let mut double = false;
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\'' if !double => single = !single,
            '"' if !single => double = !double,
            '\\' if !single => {
                if let Some(&next) = chars.peek() {
                    if next == ' ' || next == '"' || next == '\'' || next == '\\' {
                        current.push(next);
                        chars.next();
                    } else {
                        current.push(c);
                    }
                } else {
                    current.push(c);
                }
            }
            c if c.is_whitespace() && !single && !double => {
                if !current.is_empty() {
                    args.push(std::mem::take(&mut current));
                }
            }
            _ => current.push(c),
        }
    }
    if !current.is_empty() {
        args.push(current);
    }
    args
}

/// For each included header, how the translation unit uses it.
///
/// This is a heuristic based on the clang `-ast-dump=json` output: a header is
/// `NeedsDefinition` when the unit constructs a value of one of its classes or
/// instantiates a template declared in it; `DeclarationOnly` when some symbol is
/// referenced but no definition is needed; `Unused` when nothing is referenced.
pub fn header_usage_from_ast(
    ast: &[u8],
    root: Option<&str>,
    source: &str,
    headers: &[String],
) -> Vec<(String, HeaderUsage)> {
    let mut de = serde_json::Deserializer::from_slice(ast);
    let mut summary = Summary::default();
    let _ = de.deserialize_any(NodeVisitor {
        summary: &mut summary,
        root,
        source,
        headers,
        inherited: None,
    });
    classify(&summary, source, headers)
}

fn classify(summary: &Summary, source: &str, headers: &[String]) -> Vec<(String, HeaderUsage)> {
    fn types_in<'a>(list: &'a [(String, String)], file: &str) -> Vec<&'a str> {
        list.iter()
            .filter(|(f, _)| f == file)
            .map(|(_, t)| t.as_str())
            .collect()
    }
    fn names_in<'a>(list: &'a [(String, String)], file: &str) -> Vec<&'a str> {
        list.iter()
            .filter(|(f, _)| f == file)
            .map(|(_, n)| n.as_str())
            .collect()
    }

    headers
        .iter()
        .map(|header| {
            let classes = names_in(&summary.defined_classes, header);
            let functions = names_in(&summary.defined_functions, header);
            let constructed = types_in(&summary.constructed_types, source);
            let referenced_types = types_in(&summary.referenced_types, source);
            let source_decls = names_in(&summary.referenced_decls, source);
            let instantiated = summary.instantiated_files.contains(header);

            let needs_definition = instantiated || classes.iter().any(|c| constructed.contains(c));
            let referenced = instantiated
                || classes.iter().any(|c| referenced_types.contains(c))
                || functions.iter().any(|f| source_decls.contains(f));

            let usage = if needs_definition {
                HeaderUsage::NeedsDefinition
            } else if referenced {
                HeaderUsage::DeclarationOnly
            } else {
                HeaderUsage::Unused
            };
            (header.clone(), usage)
        })
        .collect()
}

/// Compact data extracted from the AST dump.
#[derive(Default)]
struct Summary {
    defined_classes: Vec<(String, String)>,   // (file, class name)
    defined_functions: Vec<(String, String)>, // (file, function name)
    referenced_types: Vec<(String, String)>,  // (file, base type) type mentions in the source
    constructed_types: Vec<(String, String)>, // (file, base type) constructed in the source
    referenced_decls: Vec<(String, String)>,  // (file, name) DeclRefExpr in the source
    instantiated_files: Vec<String>,
}

#[derive(Deserialize)]
struct LocInfo {
    #[serde(default)]
    file: Option<String>,
}

#[derive(Deserialize)]
struct TypeInfo {
    #[serde(default, rename = "qualType")]
    qual_type: Option<String>,
}

/// Seed that deserializes one AST node into its `Summary` contribution,
/// streaming its `inner` children without materializing them.
struct NodeSeed<'a> {
    summary: &'a mut Summary,
    root: Option<&'a str>,
    source: &'a str,
    headers: &'a [String],
    inherited: Option<String>,
}

impl<'de> DeserializeSeed<'de> for NodeSeed<'_> {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<(), D::Error>
    where
        D: de::Deserializer<'de>,
    {
        deserializer.deserialize_any(NodeVisitor {
            summary: self.summary,
            root: self.root,
            source: self.source,
            headers: self.headers,
            inherited: self.inherited,
        })
    }
}

struct NodeVisitor<'a> {
    summary: &'a mut Summary,
    root: Option<&'a str>,
    source: &'a str,
    headers: &'a [String],
    inherited: Option<String>,
}

impl<'de> Visitor<'de> for NodeVisitor<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("an AST node object")
    }

    fn visit_map<A>(self, mut map: A) -> Result<(), A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut kind = String::new();
        let mut name: Option<String> = None;
        let mut own_file: Option<String> = None;
        let mut qual_type: Option<String> = None;
        let mut complete = false;
        let mut is_instantiation = false;

        loop {
            let key = map.next_key::<String>()?;
            let Some(key) = key else { break };
            match key.as_str() {
                "kind" => {
                    if let Some(k) = map.next_value::<Option<String>>()? {
                        kind = k;
                    }
                }
                "name" => name = map.next_value::<Option<String>>()?,
                "loc" => {
                    let loc = map.next_value::<LocInfo>()?;
                    own_file = loc.file;
                }
                "type" => {
                    let t = map.next_value::<TypeInfo>()?;
                    qual_type = t.qual_type;
                }
                "completeDefinition" => complete = map.next_value::<bool>()?,
                "templateSpecializationKind" => {
                    is_instantiation = map.next_value::<Option<String>>()?.is_some();
                }
                "inner" => {
                    let node_file = own_file
                        .clone()
                        .or_else(|| self.inherited.clone())
                        .map(|f| normalize(self.root, &f));
                    // Only descend into nodes that belong to the translation unit
                    // or a project header; system-header subtrees are the bulk of
                    // the dump and carry nothing we care about.
                    let relevant = node_file
                        .as_deref()
                        .is_none_or(|f| f == self.source || self.headers.iter().any(|h| h == f));
                    if relevant {
                        map.next_value_seed(InnerSeed {
                            summary: &mut *self.summary,
                            root: self.root,
                            source: self.source,
                            headers: self.headers,
                            inherited: node_file,
                        })?;
                    } else {
                        map.next_value::<IgnoredAny>()?;
                    }
                }
                _ => {
                    map.next_value::<IgnoredAny>()?;
                }
            }
        }

        let file = own_file
            .or(self.inherited)
            .map(|f| normalize(self.root, &f));
        let in_headers = file
            .as_deref()
            .is_some_and(|f| self.headers.iter().any(|h| h == f));
        let in_source = file.as_deref() == Some(self.source);
        let kind = kind.as_str();
        let name_ref = name.as_deref().unwrap_or("");
        let qual_ref = qual_type.as_deref().unwrap_or("");

        if let Some(file) = file.as_deref() {
            if (kind == "CXXRecordDecl" || kind == "RecordDecl")
                && complete
                && in_headers
                && !name_ref.is_empty()
            {
                self.summary
                    .defined_classes
                    .push((file.to_string(), name_ref.to_string()));
            }
            if (kind == "FunctionDecl" || kind == "CXXMethodDecl")
                && in_headers
                && !name_ref.is_empty()
            {
                self.summary
                    .defined_functions
                    .push((file.to_string(), name_ref.to_string()));
            }
            let is_constructed = matches!(
                kind,
                "CXXConstructExpr"
                    | "CXXNewExpr"
                    | "CXXTemporaryObjectExpr"
                    | "CXXFunctionalCastExpr"
            );
            let is_type_mention = is_constructed
                || matches!(
                    kind,
                    "VarDecl" | "ParmVarDecl" | "FieldDecl" | "DeclRefExpr"
                );
            if in_source {
                if is_constructed && !qual_ref.is_empty() {
                    self.summary
                        .constructed_types
                        .push((file.to_string(), base_type_name(qual_ref)));
                }
                if is_type_mention && !qual_ref.is_empty() {
                    self.summary
                        .referenced_types
                        .push((file.to_string(), base_type_name(qual_ref)));
                }
                if kind == "DeclRefExpr" && !name_ref.is_empty() {
                    self.summary
                        .referenced_decls
                        .push((file.to_string(), name_ref.to_string()));
                }
            }
            if (kind == "CXXRecordDecl" || kind == "FunctionDecl") && is_instantiation && in_headers
            {
                self.summary.instantiated_files.push(file.to_string());
            }
        }
        Ok(())
    }
}

/// Seed that deserializes an `inner` array, processing each child node in turn.
struct InnerSeed<'a> {
    summary: &'a mut Summary,
    root: Option<&'a str>,
    source: &'a str,
    headers: &'a [String],
    inherited: Option<String>,
}

impl<'de> DeserializeSeed<'de> for InnerSeed<'_> {
    type Value = ();

    fn deserialize<D>(self, deserializer: D) -> Result<(), D::Error>
    where
        D: de::Deserializer<'de>,
    {
        deserializer.deserialize_seq(InnerVisitor {
            summary: self.summary,
            root: self.root,
            source: self.source,
            headers: self.headers,
            inherited: self.inherited,
        })
    }
}

struct InnerVisitor<'a> {
    summary: &'a mut Summary,
    root: Option<&'a str>,
    source: &'a str,
    headers: &'a [String],
    inherited: Option<String>,
}

impl<'de> Visitor<'de> for InnerVisitor<'_> {
    type Value = ();

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("an inner AST node array")
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<(), A::Error>
    where
        A: SeqAccess<'de>,
    {
        while let Some(()) = seq.next_element_seed(NodeSeed {
            summary: &mut *self.summary,
            root: self.root,
            source: self.source,
            headers: self.headers,
            inherited: self.inherited.clone(),
        })? {}
        Ok(())
    }
}

/// Normalize a clang `loc.file` to the analysis form: relative to `root` for
/// project files, absolute otherwise.
fn normalize(root: Option<&str>, file: &str) -> String {
    match root {
        Some(root) => file
            .strip_prefix(root)
            .and_then(|rest| rest.strip_prefix('/'))
            .map(|rest| rest.to_string())
            .unwrap_or_else(|| file.to_string()),
        None => file.to_string(),
    }
}

/// Reduce a clang type string to its base class name, e.g. `class Foo *` -> `Foo`
/// and `std::vector<int>` -> `vector`.
fn base_type_name(t: &str) -> String {
    let mut s = t.rsplit("::").next().unwrap_or(t).to_string();
    for prefix in ["class ", "struct ", "enum ", "union "] {
        if let Some(rest) = s.strip_prefix(prefix) {
            s = rest.to_string();
            break;
        }
    }
    while s.starts_with('*') || s.starts_with('&') {
        s.remove(0);
    }
    while s.ends_with('*') || s.ends_with('&') {
        s.pop();
    }
    if let Some(i) = s.find('<') {
        s.truncate(i);
    }
    s.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn run(ast: serde_json::Value, source: &str, headers: &[&str]) -> Vec<(String, HeaderUsage)> {
        let headers: Vec<String> = headers.iter().map(|s| s.to_string()).collect();
        header_usage_from_ast(&ast.to_string().into_bytes(), None, source, &headers)
    }

    #[test]
    fn constructed_class_needs_definition() {
        let ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [
                { "kind": "CXXRecordDecl", "name": "Bar", "completeDefinition": true,
                  "loc": { "file": "src/bar.h", "line": 1, "col": 1 } },
                { "kind": "CXXConstructExpr", "type": { "qualType": "Bar" },
                  "loc": { "file": "src/main.cpp", "line": 2, "col": 1 } }
            ]
        });
        assert_eq!(
            run(ast, "src/main.cpp", &["src/bar.h"]),
            vec![("src/bar.h".to_string(), HeaderUsage::NeedsDefinition)]
        );
    }

    #[test]
    fn pointer_use_is_declaration_only() {
        let ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [
                { "kind": "CXXRecordDecl", "name": "Foo", "completeDefinition": true,
                  "loc": { "file": "src/foo.h", "line": 1, "col": 1 } },
                { "kind": "ParmVarDecl", "name": "p", "type": { "qualType": "class Foo *" },
                  "loc": { "file": "src/main.cpp", "line": 2, "col": 1 } }
            ]
        });
        assert_eq!(
            run(ast, "src/main.cpp", &["src/foo.h"]),
            vec![("src/foo.h".to_string(), HeaderUsage::DeclarationOnly)]
        );
    }

    #[test]
    fn function_call_is_declaration_only() {
        let ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [
                { "kind": "FunctionDecl", "name": "foo_size",
                  "loc": { "file": "src/foo.h", "line": 1, "col": 1 } },
                { "kind": "DeclRefExpr", "name": "foo_size",
                  "loc": { "file": "src/main.cpp", "line": 2, "col": 1 } }
            ]
        });
        assert_eq!(
            run(ast, "src/main.cpp", &["src/foo.h"]),
            vec![("src/foo.h".to_string(), HeaderUsage::DeclarationOnly)]
        );
    }

    #[test]
    fn unused_header_is_removable() {
        let ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [
                { "kind": "CXXRecordDecl", "name": "UnusedClass", "completeDefinition": true,
                  "loc": { "file": "src/unused.h", "line": 1, "col": 1 } },
                { "kind": "FunctionDecl", "name": "main",
                  "loc": { "file": "src/main.cpp", "line": 2, "col": 1 } }
            ]
        });
        assert_eq!(
            run(ast, "src/main.cpp", &["src/unused.h"]),
            vec![("src/unused.h".to_string(), HeaderUsage::Unused)]
        );
    }

    #[test]
    fn instantiated_template_requires_definition() {
        let ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [
                { "kind": "CXXRecordDecl", "name": "vector", "completeDefinition": true,
                  "templateSpecializationKind": "ImplicitInstantiation",
                  "loc": { "file": "src/vector.h", "line": 1, "col": 1 } }
            ]
        });
        assert_eq!(
            run(ast, "src/main.cpp", &["src/vector.h"]),
            vec![("src/vector.h".to_string(), HeaderUsage::NeedsDefinition)]
        );
    }

    #[test]
    fn construct_in_header_is_unused_by_source() {
        // The construct happens in foo.h's own inline function, not in the TU.
        let ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [
                { "kind": "CXXRecordDecl", "name": "Foo", "completeDefinition": true,
                  "loc": { "file": "src/foo.h", "line": 1, "col": 1 } },
                { "kind": "FunctionDecl", "name": "make",
                  "loc": { "file": "src/foo.h", "line": 3, "col": 1 },
                  "inner": [
                      { "kind": "CXXConstructExpr", "type": { "qualType": "Foo" },
                        "loc": { "file": "src/foo.h", "line": 4, "col": 1 } }
                  ] }
            ]
        });
        assert_eq!(
            run(ast, "src/main.cpp", &["src/foo.h"]),
            vec![("src/foo.h".to_string(), HeaderUsage::Unused)]
        );
    }

    #[test]
    fn base_type_name_strips_qualifiers() {
        assert_eq!(base_type_name("class Foo *"), "Foo");
        assert_eq!(base_type_name("std::vector<int>"), "vector");
        assert_eq!(base_type_name("Foo"), "Foo");
    }
}

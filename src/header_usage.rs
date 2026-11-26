//! Classifies how each translation unit uses its included headers.
//!
//! The compiler is asked for the real direct includes (`-H`), then the AST
//! is walked to collect symbol references and map them back to the headers
//! that provide those symbols. A header is `Unused` when nothing from it is
//! referenced. A used header is `DeclarationOnly` unless one of its
//! references needs the definition (construction, member access,
//! inline/function-template body, template instantiation, and so on).

use std::collections::HashMap;
use std::fs;
use std::io::{BufReader, Cursor, Read};
use std::path::Path;
use std::process::{Child, Command, Stdio};

use serde::de::{
    self, DeserializeSeed, Deserializer as _, IgnoredAny, MapAccess, SeqAccess, Visitor,
};
use serde::{Deserialize, Serialize};

/// How a translation unit uses a header it directly includes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HeaderUsage {
    /// No symbol from the header is referenced; the `#include` can be removed.
    Unused,
    /// Only declarations are used. The include can be replaced by forward
    /// declarations of the used symbols, but transitive types the unit uses
    /// still need direct includes.
    DeclarationOnly,
    /// A definition-requiring use exists (construction / instantiation).
    NeedsDefinition,
}

/// Run clang on a single translation unit and return, for each directly
/// included project header, how the unit uses it.
///
/// Direct includes are obtained from the compiler's `-H` output, using the
/// compiler and flags recorded in `compile_commands.json`. The resulting paths
/// are in the analysis's stored form: relative to the project root for project
/// files, absolute for others. Only project headers are classified.
pub fn analyze_unit_header_usage(
    source: &str,
    root: Option<&str>,
    compile_commands: &Option<CompileCommands>,
) -> Vec<(String, HeaderUsage)> {
    let invocation = Invocation::for_source(source, root, compile_commands);
    let headers = invocation.direct_includes(root);
    if headers.is_empty() {
        return Vec::new();
    }
    let Some((mut child, summary)) = invocation.dump_ast(root, &headers) else {
        return Vec::new();
    };
    let success = child.wait().ok().is_some_and(|s| s.success());
    if !success {
        log::debug!("header usage: AST dump for {} failed", invocation.source);
        return Vec::new();
    }
    let usage = classify(&summary, &headers);
    log::debug!("header usage for {}: {usage:?}", invocation.source);
    usage
}

/// Compile commands for the build, loaded from `compile_commands.json` in the
/// build directory (or its nearest ancestor that has one).
pub struct CompileCommands {
    entries: Vec<CompileCommand>,
}

impl CompileCommands {
    /// Load the compile commands from the build directory.
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

impl CompileCommand {
    fn argv(&self) -> Vec<String> {
        self.arguments
            .clone()
            .unwrap_or_else(|| shell_split(self.command.as_deref().unwrap_or_default()))
    }
}

/// One compiler invocation for a translation unit: the compiler, its working
/// directory, and the compile flags without the source/output arguments.
struct Invocation {
    compiler: String,
    directory: String,
    flags: Vec<String>,
    /// Absolute source path as passed to the compiler.
    source: String,
}

impl Invocation {
    fn for_source(
        source: &str,
        root: Option<&str>,
        compile_commands: &Option<CompileCommands>,
    ) -> Self {
        let absolute_source = if Path::new(source).is_absolute() {
            source.to_string()
        } else {
            match root {
                Some(root) => format!("{root}/{source}"),
                None => source.to_string(),
            }
        };

        let Some(entry) = compile_commands
            .as_ref()
            .and_then(|cc| cc.for_source(&absolute_source))
        else {
            return Self {
                compiler: std::env::var("CXX").unwrap_or_else(|_| "clang++".to_string()),
                directory: root.unwrap_or_default().to_string(),
                flags: vec!["-std=c++17".to_string()],
                source: absolute_source,
            };
        };

        // Keep the compiler (argv[0]) intact: it may be a wrapper that sets up
        // include paths, a version manager shim, or ccache.
        let mut argv = entry.argv().into_iter();
        let compiler = argv
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "clang++".to_string());

        let mut flags = Vec::new();
        let mut skip_next = false;
        for arg in argv {
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
                // The analyzer re-runs the compiler for header analysis; tracing
                // those runs would drop stray "-.json" files in the build dir.
                _ if arg.starts_with("-ftime-trace") => continue,
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

        Self {
            compiler,
            directory: entry.directory.clone(),
            flags,
            source: absolute_source,
        }
    }

    /// Returns the project headers directly included by the source, in
    /// analysis form (relative paths for project headers).
    fn direct_includes(&self, root: Option<&str>) -> Vec<String> {
        let mut cmd = Command::new(&self.compiler);
        cmd.current_dir(&self.directory);
        cmd.args(&self.flags);
        cmd.args(["-E", "-H", &self.source]);
        cmd.stdout(Stdio::null()).stderr(Stdio::piped());

        let Ok(output) = cmd.output() else {
            return Vec::new();
        };
        if !output.status.success() {
            log::debug!(
                "header usage: preprocessing {} failed with {}",
                self.source,
                output.status
            );
            return Vec::new();
        }
        let includes = parse_direct_includes(&output.stderr, &self.directory, root);
        log::debug!(
            "header usage: direct includes for {}: {includes:?}",
            self.source
        );
        includes
    }

    /// Spawn clang and stream its `-ast-dump=json` stdout into a compact
    /// `Summary`. The (potentially hundreds of MB) dump is consumed
    /// incrementally and never materialized in memory.
    fn dump_ast(&self, root: Option<&str>, headers: &[String]) -> Option<(Child, Summary)> {
        let mut cmd = Command::new(&self.compiler);
        cmd.current_dir(&self.directory);
        cmd.args(&self.flags);
        cmd.args(["-fsyntax-only", "-Xclang", "-ast-dump=json", &self.source]);
        cmd.stdout(Stdio::piped()).stderr(Stdio::null());

        let mut child = cmd.spawn().ok()?;
        let stdout = child.stdout.take()?;
        let mut reader = BufReader::new(stdout);

        // Skip any diagnostics before the JSON document (which starts at the
        // first '{'); on a clean compile there are none.
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
        let source = normalize(root, &self.source);
        let _ = de.deserialize_any(NodeVisitor {
            summary: &mut summary,
            root,
            source: &source,
            headers,
            inherited: None,
        });
        Some((child, summary))
    }
}

/// Extracts the depth-1 entries from clang's `-H` output.
fn parse_direct_includes(stderr: &[u8], directory: &str, root: Option<&str>) -> Vec<String> {
    let text = String::from_utf8_lossy(stderr);
    let mut includes = Vec::new();
    for line in text.lines() {
        let Some(path) = line.strip_prefix(". ") else {
            continue;
        };
        let path = path.trim();
        if path.is_empty() || path.starts_with('<') {
            continue;
        }
        let absolute = if Path::new(path).is_absolute() {
            path.to_string()
        } else {
            format!("{directory}/{path}")
        };
        let absolute = clean_absolute_path(&absolute);
        let normalized = normalize(root, &absolute);
        // Only project headers, not system or out-of-project headers.
        if normalized.starts_with('/') || normalized.is_empty() {
            continue;
        }
        if !includes.contains(&normalized) {
            includes.push(normalized);
        }
    }
    includes
}

/// Removes `.` and `..` components from an absolute path without touching the
/// filesystem, so include paths from `-H` line up with AST locations.
fn clean_absolute_path(path: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            part => parts.push(part),
        }
    }
    format!("/{}", parts.join("/"))
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

/// For each header, how the translation unit uses it. This is a convenience
/// entry point for tests and callers that already have an AST dump.
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
    classify(&summary, headers)
}

/// Compact data extracted from the AST dump.
#[derive(Default)]
struct Summary {
    /// Last file name written by clang's JSON dumper. Clang omits `loc.file`
    /// when it is unchanged, so this restores the omitted file.
    last_file: Option<String>,
    /// Declarations in the source or a directly included header, by AST id.
    decls: HashMap<String, DeclInfo>,
    /// Class/alias names declared in directly included headers.
    header_types: HashMap<String, Vec<String>>,
    /// Class names *defined* in directly included headers.
    header_type_definitions: HashMap<String, Vec<String>>,
    /// All declaration names declared in directly included headers.
    header_symbols: HashMap<String, Vec<String>>,
    /// Names defined (not merely declared) in directly included headers.
    header_definitions: HashMap<String, Vec<String>>,
    /// Function keys (mangled name when available) declared in headers.
    header_function_keys: HashMap<String, String>,
    /// Function keys defined by the source (to match header declarations).
    source_function_definitions: Vec<String>,
    /// References to declarations, from the source.
    decl_refs: Vec<DeclReference>,
    /// Type mentions from the source.
    type_refs: Vec<TypeReference>,
    /// Headers whose templates were instantiated in this TU.
    instantiated_files: Vec<String>,
    /// Headers referenced through macro expansions in the source.
    macro_refs: Vec<String>,
}

#[derive(Clone, Debug)]
struct DeclInfo {
    file: String,
    is_definition: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DeclKind {
    Class,
    Function,
    Variable,
    Other,
}

struct DeclReference {
    id: Option<String>,
    name: String,
}

struct TypeReference {
    name: String,
    needs_definition: bool,
}

/// A source location with the file clang's JSON writer omitted restored.
#[derive(Clone, Default)]
struct ResolvedLoc {
    /// File where the declaration/entity is spelled (macro definition for a
    /// macro expansion).
    spelling: Option<String>,
    /// File where the entity is used (macro expansion site for a macro).
    expansion: Option<String>,
    is_macro: bool,
}

#[derive(Deserialize, Default)]
struct LocInfo {
    #[serde(default)]
    file: Option<String>,
    #[serde(default, rename = "spellingLoc")]
    spelling_loc: Option<Box<LocInfo>>,
    #[serde(default, rename = "expansionLoc")]
    expansion_loc: Option<Box<LocInfo>>,
}

#[derive(Deserialize, Default)]
struct RangeInfo {
    #[serde(default)]
    begin: LocInfo,
    #[serde(default)]
    end: LocInfo,
}

fn process_loc(loc: &LocInfo, last_file: &mut Option<String>) -> ResolvedLoc {
    if let (Some(spelling), Some(expansion)) = (&loc.spelling_loc, &loc.expansion_loc) {
        // clang writes the spelling location first, then the expansion one.
        let spelling = process_loc(spelling, last_file);
        let expansion = process_loc(expansion, last_file);
        ResolvedLoc {
            spelling: spelling.spelling.or(spelling.expansion),
            expansion: expansion.expansion.or(expansion.spelling),
            is_macro: true,
        }
    } else {
        if let Some(file) = &loc.file {
            *last_file = Some(file.clone());
        }
        let file = loc.file.clone().or_else(|| last_file.clone());
        ResolvedLoc {
            spelling: file.clone(),
            expansion: file,
            is_macro: false,
        }
    }
}

#[derive(Clone, Default, Deserialize)]
struct TypeInfo {
    #[serde(default, rename = "qualType")]
    qual_type: Option<String>,
    #[serde(default, rename = "desugaredQualType")]
    desugared_qual_type: Option<String>,
}

#[derive(Deserialize)]
struct ReferencedDeclInfo {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

#[derive(Default)]
struct ChildInfo {
    kind: String,
    type_info: Option<TypeInfo>,
}

#[derive(Default)]
struct InnerInfo {
    first_type: Option<TypeInfo>,
    has_body: bool,
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
    type Value = ChildInfo;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
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
    type Value = ChildInfo;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("an AST node object")
    }

    fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut kind = String::new();
        let mut id: Option<String> = None;
        let mut name: Option<String> = None;
        let mut mangled_name: Option<String> = None;
        let mut loc: Option<ResolvedLoc> = None;
        let mut qual_type: Option<String> = None;
        let mut desugared_qual_type: Option<String> = None;
        let mut arg_type: Option<TypeInfo> = None;
        let mut referenced: Option<ReferencedDeclInfo> = None;
        let mut complete = false;
        let mut is_instantiation = false;
        let mut storage_class: Option<String> = None;
        let mut inner_info = InnerInfo::default();

        loop {
            let Some(key) = map.next_key::<String>()? else {
                break;
            };
            match key.as_str() {
                "kind" => {
                    if let Some(k) = map.next_value::<Option<String>>()? {
                        kind = k;
                    }
                }
                "id" => id = map.next_value::<Option<String>>()?,
                "name" => name = map.next_value::<Option<String>>()?,
                "mangledName" => mangled_name = map.next_value::<Option<String>>()?,
                "loc" => {
                    if let Some(value) = map.next_value::<Option<LocInfo>>()? {
                        loc = Some(process_loc(&value, &mut self.summary.last_file));
                    }
                }
                "range" => {
                    if let Some(value) = map.next_value::<Option<RangeInfo>>()? {
                        // clang also updates its file deduplication state for
                        // ranges, so keep it in sync.
                        let _ = process_loc(&value.begin, &mut self.summary.last_file);
                        let _ = process_loc(&value.end, &mut self.summary.last_file);
                    }
                }
                "type" => {
                    if let Some(t) = map.next_value::<Option<TypeInfo>>()? {
                        qual_type = t.qual_type;
                        desugared_qual_type = t.desugared_qual_type;
                    }
                }
                "argType" => {
                    arg_type = map.next_value::<Option<TypeInfo>>()?;
                }
                "referencedDecl" => {
                    referenced = map.next_value::<Option<ReferencedDeclInfo>>()?;
                }
                "completeDefinition" => complete = map.next_value::<bool>()?,
                "templateSpecializationKind" => {
                    is_instantiation = map.next_value::<Option<String>>()?.is_some();
                }
                "storageClass" => storage_class = map.next_value::<Option<String>>()?,
                "inner" => {
                    let current_file = current_file(&loc, &self.inherited);
                    let normalized = current_file.as_deref().map(|f| normalize(self.root, f));
                    let relevant = normalized
                        .as_deref()
                        .is_none_or(|f| f == self.source || self.headers.iter().any(|h| h == f));
                    if relevant {
                        let inherited = current_file;
                        inner_info = map.next_value_seed(InnerSeed {
                            summary: &mut *self.summary,
                            root: self.root,
                            source: self.source,
                            headers: self.headers,
                            inherited,
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

        let decl_file_raw = loc
            .as_ref()
            .and_then(|l| l.spelling.clone().or_else(|| l.expansion.clone()))
            .or_else(|| self.inherited.clone());
        let use_file_raw = loc
            .as_ref()
            .and_then(|l| l.expansion.clone().or_else(|| l.spelling.clone()))
            .or_else(|| self.inherited.clone());
        let decl_file = decl_file_raw.as_deref().map(|f| normalize(self.root, f));
        let use_file = use_file_raw.as_deref().map(|f| normalize(self.root, f));

        let in_source = use_file.as_deref() == Some(self.source);
        let declared_in_source = decl_file.as_deref() == Some(self.source);
        let in_headers = decl_file
            .as_deref()
            .is_some_and(|f| self.headers.iter().any(|h| h == f));

        // A macro expanded in the source whose definition lives in a direct
        // header is a use of that header.
        if loc.as_ref().is_some_and(|l| l.is_macro)
            && in_source
            && let Some(file) = &decl_file
            && in_headers
        {
            self.summary.macro_refs.push(file.clone());
        }

        let name_ref = name.as_deref().unwrap_or_default();
        let kind_ref = kind.as_str();
        let decl_kind = decl_kind(kind_ref);
        let is_definition = match decl_kind {
            DeclKind::Class => complete,
            DeclKind::Function => inner_info.has_body,
            DeclKind::Variable => storage_class.as_deref() != Some("extern"),
            DeclKind::Other => false,
        };
        let function_key = mangled_name.as_deref().unwrap_or(name_ref);

        if kind_ref.ends_with("Decl")
            && (declared_in_source || in_headers)
            && let Some(decl_id) = &id
            && !name_ref.is_empty()
        {
            self.summary.decls.insert(
                decl_id.clone(),
                DeclInfo {
                    file: decl_file.clone().unwrap_or_default(),
                    is_definition,
                },
            );
        }

        if in_headers
            && !name_ref.is_empty()
            && matches!(
                decl_kind,
                DeclKind::Class | DeclKind::Function | DeclKind::Variable
            )
        {
            push_unique(
                self.summary
                    .header_symbols
                    .entry(name_ref.to_string())
                    .or_default(),
                decl_file.as_deref().unwrap_or_default(),
            );
            if decl_kind == DeclKind::Class {
                let file = decl_file.as_deref().unwrap_or_default();
                push_unique(
                    self.summary
                        .header_types
                        .entry(name_ref.to_string())
                        .or_default(),
                    file,
                );
                if complete {
                    push_unique(
                        self.summary
                            .header_type_definitions
                            .entry(name_ref.to_string())
                            .or_default(),
                        file,
                    );
                }
            }
            if is_definition {
                push_unique(
                    self.summary
                        .header_definitions
                        .entry(name_ref.to_string())
                        .or_default(),
                    decl_file.as_deref().unwrap_or_default(),
                );
            }
            if decl_kind == DeclKind::Function {
                if !function_key.is_empty() {
                    self.summary
                        .header_function_keys
                        .entry(function_key.to_string())
                        .or_insert_with(|| decl_file.clone().unwrap_or_default());
                }
                if !name_ref.is_empty() {
                    self.summary
                        .header_function_keys
                        .entry(name_ref.to_string())
                        .or_insert_with(|| decl_file.clone().unwrap_or_default());
                }
            }
            if is_instantiation
                && matches!(decl_kind, DeclKind::Class | DeclKind::Function)
                && let Some(file) = &decl_file
            {
                self.summary.instantiated_files.push(file.clone());
            }
        }

        if declared_in_source
            && decl_kind == DeclKind::Function
            && inner_info.has_body
            && !function_key.is_empty()
        {
            self.summary
                .source_function_definitions
                .push(function_key.to_string());
        }

        if in_source {
            if kind_ref == "DeclRefExpr" {
                self.summary.decl_refs.push(DeclReference {
                    id: referenced.as_ref().and_then(|r| r.id.clone()),
                    name: referenced
                        .as_ref()
                        .and_then(|r| r.name.clone())
                        .unwrap_or_else(|| name_ref.to_string()),
                });
            }

            match kind_ref {
                "CXXConstructExpr"
                | "CXXNewExpr"
                | "CXXTemporaryObjectExpr"
                | "CXXFunctionalCastExpr"
                | "CXXDeleteExpr"
                | "InitListExpr" => {
                    add_type_refs(
                        self.summary,
                        qual_type.as_deref(),
                        desugared_qual_type.as_deref(),
                        true,
                    );
                }
                "MemberExpr" => {
                    if let Some(base) = &inner_info.first_type {
                        add_type_refs(
                            self.summary,
                            base.qual_type.as_deref(),
                            base.desugared_qual_type.as_deref(),
                            true,
                        );
                    }
                }
                "VarDecl" | "FieldDecl" => {
                    let is_indirect = [qual_type.as_deref(), desugared_qual_type.as_deref()]
                        .into_iter()
                        .flatten()
                        .any(|t| t.contains('*') || t.contains('&'));
                    add_type_refs(
                        self.summary,
                        qual_type.as_deref(),
                        desugared_qual_type.as_deref(),
                        !is_indirect,
                    );
                }
                "UnaryExprOrTypeTraitExpr" => {
                    if let Some(arg) = &arg_type {
                        add_type_refs(
                            self.summary,
                            arg.qual_type.as_deref(),
                            arg.desugared_qual_type.as_deref(),
                            true,
                        );
                    }
                }
                "ParmVarDecl" | "DeclRefExpr" => {
                    add_type_refs(
                        self.summary,
                        qual_type.as_deref(),
                        desugared_qual_type.as_deref(),
                        false,
                    );
                }
                _ => {}
            }
        }

        Ok(ChildInfo {
            kind,
            type_info: if qual_type.is_some() || desugared_qual_type.is_some() {
                Some(TypeInfo {
                    qual_type,
                    desugared_qual_type,
                })
            } else {
                None
            },
        })
    }
}

fn current_file(loc: &Option<ResolvedLoc>, inherited: &Option<String>) -> Option<String> {
    loc.as_ref()
        .and_then(|l| l.spelling.clone().or_else(|| l.expansion.clone()))
        .or_else(|| inherited.clone())
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
    type Value = InnerInfo;

    fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
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
    type Value = InnerInfo;

    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("an inner AST node array")
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut info = InnerInfo::default();
        let mut seen_first = false;
        while let Some(child) = seq.next_element_seed(NodeSeed {
            summary: &mut *self.summary,
            root: self.root,
            source: self.source,
            headers: self.headers,
            inherited: self.inherited.clone(),
        })? {
            if !seen_first {
                seen_first = true;
                info.first_type = child.type_info;
            }
            if child.kind == "CompoundStmt" {
                info.has_body = true;
            }
        }
        Ok(info)
    }
}

fn decl_kind(kind: &str) -> DeclKind {
    match kind {
        "CXXRecordDecl"
        | "RecordDecl"
        | "ClassTemplateDecl"
        | "ClassTemplateSpecializationDecl"
        | "ClassTemplatePartialSpecializationDecl"
        | "EnumDecl"
        | "TypedefDecl"
        | "TypeAliasDecl"
        | "TypeAliasTemplateDecl" => DeclKind::Class,
        "FunctionDecl"
        | "CXXMethodDecl"
        | "CXXConstructorDecl"
        | "CXXDestructorDecl"
        | "CXXConversionDecl"
        | "FunctionTemplateDecl" => DeclKind::Function,
        "VarDecl" => DeclKind::Variable,
        _ => DeclKind::Other,
    }
}

fn push_unique(values: &mut Vec<String>, value: &str) {
    if !value.is_empty() && !values.iter().any(|v| v == value) {
        values.push(value.to_string());
    }
}

fn add_type_refs(
    summary: &mut Summary,
    qual_type: Option<&str>,
    desugared_qual_type: Option<&str>,
    needs_definition: bool,
) {
    for t in [qual_type, desugared_qual_type].into_iter().flatten() {
        for type_name in type_names(t) {
            summary.type_refs.push(TypeReference {
                name: type_name,
                needs_definition,
            });
        }
    }
}

fn classify(summary: &Summary, headers: &[String]) -> Vec<(String, HeaderUsage)> {
    let mut usage: HashMap<&str, HeaderUsage> = headers
        .iter()
        .map(|h| (h.as_str(), HeaderUsage::Unused))
        .collect();

    fn mark(usage: &mut HashMap<&str, HeaderUsage>, file: &str, needs_definition: bool) {
        let Some(slot) = usage.get_mut(file) else {
            return;
        };

        *slot = match (*slot, needs_definition) {
            (HeaderUsage::NeedsDefinition, _) => HeaderUsage::NeedsDefinition,
            (_, true) => HeaderUsage::NeedsDefinition,
            (HeaderUsage::DeclarationOnly, false) => HeaderUsage::DeclarationOnly,
            (HeaderUsage::Unused, false) => HeaderUsage::DeclarationOnly,
        };
    }

    for reference in &summary.decl_refs {
        if let Some(info) = reference.id.as_ref().and_then(|id| summary.decls.get(id)) {
            if !info.file.is_empty() {
                mark(&mut usage, &info.file, info.is_definition);
            }
            continue;
        }
        // Fallback for ASTs without declaration ids (and for synthetic tests).
        if let Some(files) = summary.header_symbols.get(&reference.name) {
            let definitions = summary.header_definitions.get(&reference.name);
            for file in files {
                let needs_definition =
                    definitions.is_some_and(|files| files.iter().any(|f| f == file));
                mark(&mut usage, file, needs_definition);
            }
        }
    }

    for reference in &summary.type_refs {
        if let Some(files) = summary.header_types.get(&reference.name) {
            let definitions = summary.header_type_definitions.get(&reference.name);
            for file in files {
                let needs_definition = reference.needs_definition
                    && definitions.is_some_and(|files| files.iter().any(|f| f == file));
                mark(&mut usage, file, needs_definition);
            }
        }
    }

    for key in &summary.source_function_definitions {
        if let Some(file) = summary.header_function_keys.get(key) {
            mark(&mut usage, file, false);
        }
    }

    for file in &summary.instantiated_files {
        mark(&mut usage, file, true);
    }
    for file in &summary.macro_refs {
        mark(&mut usage, file, false);
    }

    headers
        .iter()
        .map(|header| {
            (
                header.clone(),
                *usage.get(header.as_str()).unwrap_or(&HeaderUsage::Unused),
            )
        })
        .collect()
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

/// Extract the class-like names mentioned in a clang type string.
///
/// Qualifiers, pointer/reference operators, elaborated-type keywords and
/// namespace/class qualification prefixes are dropped, so `const Foo &`,
/// `class Foo *` and `std::vector<Foo>` yield `Foo` (and `vector` for the
/// latter). This also picks up types nested inside template arguments, which
/// matters for headers only referenced through e.g. `std::vector<Foo>`.
fn type_names(t: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let bytes = t.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
            let start = i;
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            let ident = &t[start..i];
            // A name immediately followed by `::` only qualifies the real type
            // name (`std` in `std::vector`, `geom` in `geom::Foo`).
            let mut next = i;
            while next < bytes.len() && (bytes[next] as char).is_whitespace() {
                next += 1;
            }
            if t.get(next..).is_some_and(|rest| rest.starts_with("::")) {
                continue;
            }
            if !is_ignored_type_name(ident) && !names.iter().any(|n| n == ident) {
                names.push(ident.to_string());
            }
        } else {
            i += 1;
        }
    }
    names
}

/// Type tokens that are keywords, qualifiers or built-in types rather than
/// project symbols.
fn is_ignored_type_name(name: &str) -> bool {
    matches!(
        name,
        "auto"
            | "bool"
            | "char"
            | "char8_t"
            | "char16_t"
            | "char32_t"
            | "class"
            | "const"
            | "consteval"
            | "constexpr"
            | "constinit"
            | "decltype"
            | "double"
            | "enum"
            | "extern"
            | "false"
            | "float"
            | "inline"
            | "int"
            | "long"
            | "mutable"
            | "noexcept"
            | "nullptr"
            | "operator"
            | "register"
            | "restrict"
            | "short"
            | "signed"
            | "static"
            | "struct"
            | "template"
            | "true"
            | "typedef"
            | "typename"
            | "typeof"
            | "union"
            | "unsigned"
            | "using"
            | "void"
            | "volatile"
            | "wchar_t"
            | "__restrict"
            | "__restrict__"
    )
}

/// Reduce a clang type string to its base class name, e.g. `class Foo *` -> `Foo`
/// and `std::vector<int>` -> `vector`.
#[cfg(test)]
fn base_type_name(t: &str) -> String {
    type_names(t)
        .into_iter()
        .next()
        .unwrap_or_else(|| t.trim().to_string())
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
    fn function_call_uses_referenced_decl_name() {
        // Real clang JSON has no `name` on DeclRefExpr; the referenced
        // declaration is nested under `referencedDecl`.
        let ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [
                { "kind": "FunctionDecl", "name": "foo_size",
                  "loc": { "file": "src/foo.h", "line": 1, "col": 1 } },
                { "kind": "DeclRefExpr",
                  "type": { "qualType": "int ()" },
                  "referencedDecl": { "kind": "FunctionDecl", "name": "foo_size" },
                  "loc": { "file": "src/main.cpp", "line": 2, "col": 1 } }
            ]
        });
        assert_eq!(
            run(ast, "src/main.cpp", &["src/foo.h"]),
            vec![("src/foo.h".to_string(), HeaderUsage::DeclarationOnly)]
        );
    }

    #[test]
    fn const_qualified_parameter_is_declaration_only() {
        let ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [
                { "kind": "CXXRecordDecl", "name": "Foo", "completeDefinition": true,
                  "loc": { "file": "src/foo.h", "line": 1, "col": 1 } },
                { "kind": "ParmVarDecl", "name": "p", "type": { "qualType": "const Foo &" },
                  "loc": { "file": "src/main.cpp", "line": 2, "col": 1 } }
            ]
        });
        assert_eq!(
            run(ast, "src/main.cpp", &["src/foo.h"]),
            vec![("src/foo.h".to_string(), HeaderUsage::DeclarationOnly)]
        );
    }

    #[test]
    fn const_qualified_construction_needs_definition() {
        let ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [
                { "kind": "CXXRecordDecl", "name": "Foo", "completeDefinition": true,
                  "loc": { "file": "src/foo.h", "line": 1, "col": 1 } },
                { "kind": "CXXConstructExpr", "type": { "qualType": "const Foo" },
                  "loc": { "file": "src/main.cpp", "line": 2, "col": 1 } }
            ]
        });
        assert_eq!(
            run(ast, "src/main.cpp", &["src/foo.h"]),
            vec![("src/foo.h".to_string(), HeaderUsage::NeedsDefinition)]
        );
    }

    #[test]
    fn member_access_needs_definition() {
        let ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [
                { "kind": "CXXRecordDecl", "name": "Foo", "completeDefinition": true,
                  "loc": { "file": "src/foo.h", "line": 1, "col": 1 } },
                { "kind": "MemberExpr", "name": "x", "type": { "qualType": "int" },
                  "loc": { "file": "src/main.cpp", "line": 2, "col": 1 },
                  "inner": [
                      { "kind": "DeclRefExpr", "type": { "qualType": "const Foo" },
                        "loc": { "file": "src/main.cpp", "line": 2, "col": 1 } }
                  ] }
            ]
        });
        assert_eq!(
            run(ast, "src/main.cpp", &["src/foo.h"]),
            vec![("src/foo.h".to_string(), HeaderUsage::NeedsDefinition)]
        );
    }

    #[test]
    fn definition_implements_header_declaration() {
        let ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [
                { "kind": "FunctionDecl", "name": "foo", "mangledName": "_Z3foov",
                  "loc": { "file": "src/foo.h", "line": 1, "col": 1 } },
                { "kind": "FunctionDecl", "name": "foo", "mangledName": "_Z3foov",
                  "loc": { "file": "src/foo.cpp", "line": 1, "col": 1 },
                  "inner": [ { "kind": "CompoundStmt" } ] }
            ]
        });
        assert_eq!(
            run(ast, "src/foo.cpp", &["src/foo.h"]),
            vec![("src/foo.h".to_string(), HeaderUsage::DeclarationOnly)]
        );
    }

    #[test]
    fn definition_with_different_mangled_name_does_not_match() {
        let ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [
                { "kind": "FunctionDecl", "name": "foo", "mangledName": "_Z3foov",
                  "loc": { "file": "src/foo.h", "line": 1, "col": 1 } },
                { "kind": "FunctionDecl", "name": "foo", "mangledName": "_ZN2ns3fooEv",
                  "loc": { "file": "src/foo.cpp", "line": 1, "col": 1 } }
            ]
        });
        assert_eq!(
            run(ast, "src/foo.cpp", &["src/foo.h"]),
            vec![("src/foo.h".to_string(), HeaderUsage::Unused)]
        );
    }

    #[test]
    fn template_argument_counts_as_type_reference() {
        let ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [
                { "kind": "CXXRecordDecl", "name": "Foo", "completeDefinition": true,
                  "loc": { "file": "src/foo.h", "line": 1, "col": 1 } },
                { "kind": "ParmVarDecl", "name": "p",
                  "type": { "qualType": "const std::vector<Foo> &" },
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
        assert_eq!(base_type_name("const Foo &"), "Foo");
        assert_eq!(base_type_name("const geom::Foo"), "Foo");
        assert_eq!(base_type_name("Foo *const"), "Foo");
        assert_eq!(base_type_name("const std::vector<Foo> &"), "vector");
    }

    #[test]
    fn type_names_extracts_nested_and_skips_qualifiers() {
        assert_eq!(
            type_names("const std::vector<geom::Foo> &"),
            vec!["vector".to_string(), "Foo".to_string()]
        );
        assert_eq!(type_names("unsigned long"), Vec::<String>::new());
        assert_eq!(type_names("void (*)(const Foo &)"), vec!["Foo".to_string()]);
    }

    #[test]
    fn aggregate_initialization_needs_definition() {
        let ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [
                { "kind": "CXXRecordDecl", "name": "Sphere", "completeDefinition": true,
                  "loc": { "file": "src/shapes.h", "line": 1, "col": 1 } },
                { "kind": "InitListExpr", "type": { "qualType": "const Sphere" },
                  "loc": { "file": "src/main.cpp", "line": 2, "col": 1 } }
            ]
        });
        assert_eq!(
            run(ast, "src/main.cpp", &["src/shapes.h"]),
            vec![("src/shapes.h".to_string(), HeaderUsage::NeedsDefinition)]
        );
    }

    #[test]
    fn inline_function_definition_needs_definition() {
        let ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [
                { "kind": "FunctionDecl", "id": "0x1", "name": "radians",
                  "loc": { "file": "src/utils.h", "line": 1, "col": 1 },
                  "inner": [ { "kind": "CompoundStmt" } ] },
                { "kind": "DeclRefExpr",
                  "loc": { "file": "src/main.cpp", "line": 2, "col": 1 },
                  "referencedDecl": { "id": "0x1", "kind": "FunctionDecl",
                                      "name": "radians" } }
            ]
        });
        assert_eq!(
            run(ast, "src/main.cpp", &["src/utils.h"]),
            vec![("src/utils.h".to_string(), HeaderUsage::NeedsDefinition)]
        );
    }

    #[test]
    fn omitted_file_uses_the_previous_file() {
        // clang's JSON dumper omits `file` when it is unchanged. A declaration
        // without `file` still belongs to the header established by the
        // previous location.
        let ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [
                { "kind": "FunctionDecl", "name": "first",
                  "loc": { "file": "src/header.h", "line": 1, "col": 1 } },
                { "kind": "FunctionDecl", "name": "second",
                  "loc": { "line": 2, "col": 1 } },
                { "kind": "DeclRefExpr",
                  "loc": { "file": "src/main.cpp", "line": 2, "col": 1 },
                  "referencedDecl": { "kind": "FunctionDecl", "name": "second" } }
            ]
        });
        assert_eq!(
            run(ast, "src/main.cpp", &["src/header.h"]),
            vec![("src/header.h".to_string(), HeaderUsage::DeclarationOnly)]
        );
    }

    #[test]
    fn parses_depth_one_includes() {
        let stderr = b".. /p/include/nested.h\n. /p/include/direct.h\n. /usr/include/stdio.h\n. /p/include/direct.h\n...\n. <built-in>\n";
        assert_eq!(
            parse_direct_includes(stderr, "/p", Some("/p")),
            vec!["include/direct.h".to_string()]
        );
    }

    #[test]
    fn parse_direct_includes_resolves_relative_paths() {
        let stderr = b". ../include/local.h\n";
        assert_eq!(
            parse_direct_includes(stderr, "/p/build", Some("/p")),
            vec!["include/local.h".to_string()]
        );
    }

    #[test]
    fn sizeof_argument_needs_definition() {
        let ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [
                { "kind": "CXXRecordDecl", "name": "Foo", "completeDefinition": true,
                  "loc": { "file": "src/foo.h", "line": 1, "col": 1 } },
                { "kind": "UnaryExprOrTypeTraitExpr", "name": "sizeof",
                  "type": { "qualType": "unsigned long" },
                  "argType": { "qualType": "Foo" },
                  "loc": { "file": "src/main.cpp", "line": 2, "col": 1 } }
            ]
        });
        assert_eq!(
            run(ast, "src/main.cpp", &["src/foo.h"]),
            vec![("src/foo.h".to_string(), HeaderUsage::NeedsDefinition)]
        );
    }

    #[test]
    fn macro_expansion_uses_the_spelling_header() {
        let ast = json!({
            "kind": "TranslationUnitDecl",
            "inner": [
                { "kind": "DeclRefExpr",
                  "loc": {
                      "spellingLoc": { "file": "src/macros.h", "line": 1, "col": 1 },
                      "expansionLoc": { "file": "src/main.cpp", "line": 2, "col": 1 }
                  } }
            ]
        });
        assert_eq!(
            run(ast, "src/main.cpp", &["src/macros.h"]),
            vec![("src/macros.h".to_string(), HeaderUsage::DeclarationOnly)]
        );
    }
}

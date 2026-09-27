use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Index into a string table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct StringId(u32);

impl StringId {
    /// Resolves the id against a string table.
    pub fn resolve(self, strings: &[String]) -> &str {
        &strings[self.0 as usize]
    }

    /// The raw index into the string table.
    pub fn index(self) -> u32 {
        self.0
    }
}

impl From<u32> for StringId {
    fn from(index: u32) -> Self {
        StringId(index)
    }
}

/// Interns strings into a shared table, returning stable `StringId`s. This
/// keeps repeated strings (unit paths, header paths, template names, inclusion
/// chains) stored exactly once throughout the analysis, instead of being
/// allocated per occurrence.
#[derive(Default)]
pub struct Interner {
    map: HashMap<String, StringId>,
    table: Vec<String>,
    root: Option<String>,
}

impl Interner {
    /// Creates an interner that relativizes paths against `root` before
    /// storing them, so repeated common prefixes are not duplicated. `root`
    /// must not have a trailing path separator.
    pub fn with_root(root: Option<String>) -> Self {
        Interner {
            map: HashMap::new(),
            table: Vec::new(),
            root: root.map(|r| r.trim_end_matches('/').to_string()),
        }
    }

    /// Returns the id for `s`, adding it to the table if not present yet.
    pub fn intern(&mut self, s: &str) -> StringId {
        if let Some(&id) = self.map.get(s) {
            return id;
        }
        let id = StringId(self.table.len() as u32);
        self.table.push(s.to_string());
        self.map.insert(s.to_string(), id);
        id
    }

    /// Interns a path, storing it relative to the configured root when possible.
    pub fn intern_path(&mut self, path: &str) -> StringId {
        self.intern(&self.relativize(path))
    }

    /// Strips a leading `root` from `path`, but only on a path-component
    /// boundary (so `/project` does not relativize `/projected/foo`).
    fn relativize(&self, path: &str) -> String {
        let Some(root) = &self.root else {
            return path.to_string();
        };
        match path.strip_prefix(root.as_str()) {
            Some(rest) if !rest.is_empty() && rest.starts_with('/') => rest[1..].to_string(),
            _ => path.to_string(),
        }
    }

    /// Resolves an id back to its interned string.
    pub fn resolve(&self, id: StringId) -> &str {
        id.resolve(&self.table)
    }

    /// Takes the string table out of a `&mut` interner, leaving it empty.
    pub fn take_table(&mut self) -> Vec<String> {
        std::mem::take(&mut self.table)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intern_path_relativizes_against_root() {
        let mut interner = Interner::with_root(Some("/project".to_string()));
        let id = interner.intern_path("/project/src/foo.h");
        assert_eq!(interner.resolve(id), "src/foo.h");
        let again = interner.intern_path("/project/src/foo.h");
        assert_eq!(again, id);
        // Paths outside the root stay absolute
        let external = interner.intern_path("/usr/include/stdio.h");
        assert_eq!(interner.resolve(external), "/usr/include/stdio.h");
    }

    #[test]
    fn intern_path_does_not_relativize_partial_component() {
        let mut interner = Interner::with_root(Some("/project".to_string()));
        let id = interner.intern_path("/projected/foo.h");
        assert_eq!(interner.resolve(id), "/projected/foo.h");
    }

    #[test]
    fn intern_path_without_root_keeps_path_unchanged() {
        let mut interner = Interner::default();
        let id = interner.intern_path("/project/src/foo.h");
        assert_eq!(interner.resolve(id), "/project/src/foo.h");
    }
}

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
}

impl Interner {
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

    /// Resolves an id back to its interned string.
    pub fn resolve(&self, id: StringId) -> &str {
        id.resolve(&self.table)
    }

    /// Consumes the interner, returning the string table (ids are indices).
    pub fn into_table(self) -> Vec<String> {
        self.table
    }
}

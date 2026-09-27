use std::{
    collections::HashMap,
    fs::File,
    io::{BufReader, BufWriter, Write},
    path::Path,
};

use serde::{Deserialize, Serialize, Serializer, ser::SerializeMap};
use thiserror::Error;

use crate::interner::StringId;
use crate::{analyzer::AnalysisResult, build_info::BuildInfo};

#[derive(Debug, Error)]
pub enum ReportError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Encode error: {0}")]
    Encode(#[from] rmp_serde::encode::Error),
    #[error("Decode error: {0}")]
    Decode(#[from] rmp_serde::decode::Error),
}

/// Borrowing view of a `HashMap` that emits its keys in sorted order.
///
/// std's `HashMap` uses a per-process random seed, so iteration order differs
/// between runs; rayon's parallel `collect()` makes it scheduling-dependent.
/// Sorting here makes the encoded bytes identical for identical inputs.
struct SortedRef<'a, K, V>(&'a HashMap<K, V>);

impl<K, V> Serialize for SortedRef<'_, K, V>
where
    K: Serialize + Ord,
    V: Serialize,
{
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut entries: Vec<(&K, &V)> = self.0.iter().collect();
        entries.sort_unstable_by(|a, b| a.0.cmp(b.0));
        let mut map = serializer.serialize_map(Some(entries.len()))?;
        for (k, v) in entries {
            map.serialize_entry(k, v)?;
        }
        map.end()
    }
}

/// `#[serde(serialize_with = "...")]` target for flat maps.
pub(crate) fn serialize_sorted_map<S, K, V>(
    map: &HashMap<K, V>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
    K: Serialize + Ord,
    V: Serialize,
{
    SortedRef(map).serialize(serializer)
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Report {
    /// String table backing the `u32` keys in `analysis`.
    pub strings: Vec<String>,
    pub build_info: BuildInfo,
    pub analysis: AnalysisResult,
}

impl Report {
    pub fn new(strings: Vec<String>, build_info: BuildInfo, analysis: AnalysisResult) -> Self {
        Self {
            strings,
            build_info,
            analysis,
        }
    }

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

pub fn write_report(report: &Report, path: &Path) -> Result<(), ReportError> {
    let file = File::create(path)?;
    let mut writer = BufWriter::new(file);
    let mut serializer = rmp_serde::Serializer::new(&mut writer).with_struct_map();
    report.serialize(&mut serializer)?;
    writer.flush()?;
    Ok(())
}

pub fn read_report(path: &Path) -> Result<Report, ReportError> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let report: Report = rmp_serde::from_read(reader)?;
    Ok(report)
}

/// Per-unit total duration differences (`target - base`), in microseconds,
/// sorted from the largest regression to the largest improvement.
pub fn unit_duration_diffs(base: &Report, target: &Report) -> Vec<(String, i64)> {
    let mut diffs: Vec<(String, i64)> = target
        .analysis
        .unit_analysis
        .iter()
        .map(|(unit, analysis)| {
            let unit_name = target.resolve(*unit);
            let base_duration = base
                .id_of(unit_name)
                .and_then(|id| base.analysis.unit_analysis.get(&id))
                .map_or(0, |a| a.total_duration);
            (
                unit_name.to_string(),
                analysis.total_duration - base_duration,
            )
        })
        .collect();
    diffs.sort_by_key(|(_, diff)| std::cmp::Reverse(*diff));
    diffs
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::{AnalysisResult, UnitAnalysis};
    use std::collections::{BTreeMap, HashMap};

    fn sample_report() -> Report {
        // Production reports carry a sorted string table where ids are positions;
        // mirror that here so `Report::id_of`'s binary search works.
        let strings = vec![
            "a.cpp".to_string(),
            "a.h".to_string(),
            "b.cpp".to_string(),
            "m.h".to_string(),
            "z.h".to_string(),
        ];
        let id_of = |s: &str| {
            StringId::from(strings.binary_search_by(|x| x.as_str().cmp(s)).unwrap() as u32)
        };
        let a_cpp = id_of("a.cpp");
        let b_cpp = id_of("b.cpp");
        let z_h = id_of("z.h");
        let a_h = id_of("a.h");
        let m_h = id_of("m.h");

        let mut unit = UnitAnalysis {
            total_duration: 100,
            frontend_duration: 80,
            backend_duration: 20,
            template_instantiation_duration: 30,
            header_parse_duration: HashMap::from([(z_h, 5), (a_h, 9)]),
            header_inclusion_paths: BTreeMap::new(),
            function_template_instantiations: HashMap::new(),
            class_template_instantiations: HashMap::new(),
        };
        unit.header_inclusion_paths
            .insert(a_h, BTreeMap::from([(vec![z_h], 3)]));
        let mut units = HashMap::new();
        units.insert(b_cpp, unit);
        units.insert(
            a_cpp,
            UnitAnalysis {
                total_duration: 50,
                frontend_duration: 40,
                backend_duration: 10,
                template_instantiation_duration: 5,
                header_parse_duration: HashMap::from([(m_h, 7)]),
                header_inclusion_paths: BTreeMap::new(),
                function_template_instantiations: HashMap::new(),
                class_template_instantiations: HashMap::new(),
            },
        );
        Report::new(strings, BuildInfo::default(), AnalysisResult::from(units))
    }

    fn encode(report: &Report) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut ser = rmp_serde::Serializer::new(&mut buf).with_struct_map();
        report.serialize(&mut ser).unwrap();
        buf
    }

    #[test]
    fn serialization_is_byte_stable() {
        let report = sample_report();
        assert_eq!(encode(&report), encode(&report));
    }

    #[test]
    fn write_then_read_round_trips() {
        let report = sample_report();
        let path = std::env::temp_dir().join("roundtrip.msgpack");
        write_report(&report, &path).unwrap();
        let read = read_report(&path).unwrap();
        assert_eq!(read.analysis.unit_analysis.len(), 2);
        let a_cpp = read.id_of("a.cpp").unwrap();
        let m_h = read.id_of("m.h").unwrap();
        assert_eq!(
            read.analysis.unit_analysis[&a_cpp].header_parse_duration[&m_h],
            7
        );
        std::fs::remove_file(path).ok();
    }

    fn report_with_unit_durations(durations: &[(&str, i64)]) -> Report {
        let mut strings: Vec<String> = durations.iter().map(|(n, _)| (*n).to_string()).collect();
        strings.sort();
        strings.dedup();
        let id_of = |s: &str| {
            StringId::from(strings.binary_search_by(|x| x.as_str().cmp(s)).unwrap() as u32)
        };
        let mut units = HashMap::new();
        for (name, duration) in durations {
            units.insert(
                id_of(name),
                UnitAnalysis {
                    total_duration: *duration,
                    frontend_duration: 0,
                    backend_duration: 0,
                    template_instantiation_duration: 0,
                    header_parse_duration: HashMap::new(),
                    header_inclusion_paths: BTreeMap::new(),
                    function_template_instantiations: HashMap::new(),
                    class_template_instantiations: HashMap::new(),
                },
            );
        }
        Report::new(strings, BuildInfo::default(), AnalysisResult::from(units))
    }

    #[test]
    fn unit_duration_diffs_orders_by_regression() {
        let base = report_with_unit_durations(&[("a.cpp", 100), ("b.cpp", 200)]);
        let target = report_with_unit_durations(&[("a.cpp", 160), ("b.cpp", 180), ("c.cpp", 50)]);

        let diffs = unit_duration_diffs(&base, &target);

        assert_eq!(diffs.len(), 3);
        assert_eq!(diffs[0], ("a.cpp".to_string(), 60));
        assert_eq!(diffs[1], ("c.cpp".to_string(), 50));
        assert_eq!(diffs[2], ("b.cpp".to_string(), -20));
    }
}

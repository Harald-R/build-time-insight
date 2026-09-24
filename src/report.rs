use std::{
    collections::HashMap,
    fs::File,
    io::{BufReader, BufWriter, Write},
    path::Path,
};

use serde::{Deserialize, Serialize, Serializer, ser::SerializeMap};
use thiserror::Error;

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

/// `serialize_with` target for the two-level `header_inclusion_paths`
/// map (`header -> inclusion chain -> count`); both levels are sorted.
pub(crate) fn serialize_sorted_nested_map<S>(
    map: &HashMap<String, HashMap<String, i64>>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    let mut outer: Vec<(&String, &HashMap<String, i64>)> = map.iter().collect();
    outer.sort_unstable_by(|a, b| a.0.cmp(b.0));

    let mut map_ser = serializer.serialize_map(Some(outer.len()))?;
    for (k, v) in outer {
        map_ser.serialize_entry(k, &SortedRef(v))?;
    }
    map_ser.end()
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Report {
    pub build_info: BuildInfo,
    pub analysis: AnalysisResult,
}

impl Report {
    pub fn new(build_info: BuildInfo, analysis: AnalysisResult) -> Self {
        Self {
            build_info,
            analysis,
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyzer::{AnalysisResult, UnitAnalysis};
    use std::collections::HashMap;

    fn sample_report() -> Report {
        let mut unit = UnitAnalysis {
            total_duration: 100,
            frontend_duration: 80,
            backend_duration: 20,
            template_instantiation_duration: 30,
            header_parse_duration: HashMap::from([("z.h".into(), 5), ("a.h".into(), 9)]),
            header_inclusion_paths: HashMap::new(),
            function_template_instantiations: HashMap::new(),
            class_template_instantiations: HashMap::new(),
        };
        unit.header_inclusion_paths
            .insert("a.h".into(), HashMap::from([("a.h -> z.h".into(), 3)]));
        let mut units = HashMap::new();
        units.insert("b.cpp".into(), unit);
        units.insert(
            "a.cpp".into(),
            UnitAnalysis {
                total_duration: 50,
                frontend_duration: 40,
                backend_duration: 10,
                template_instantiation_duration: 5,
                header_parse_duration: HashMap::from([("m.h".into(), 7)]),
                header_inclusion_paths: HashMap::new(),
                function_template_instantiations: HashMap::new(),
                class_template_instantiations: HashMap::new(),
            },
        );
        Report::new(BuildInfo::default(), AnalysisResult::from(units))
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
        assert_eq!(
            read.analysis.unit_analysis["a.cpp"].header_parse_duration["m.h"],
            7
        );
        std::fs::remove_file(path).ok();
    }
}

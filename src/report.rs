//! Serializes and deserializes the portable analysis report.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs::File,
    io::{BufReader, BufWriter},
    path::Path,
};

use flate2::{Compression, read::GzDecoder, write::GzEncoder};
use serde::{Deserialize, Serialize, Serializer, ser::SerializeMap};
use thiserror::Error;

use crate::interner::StringId;
use crate::{
    analyzer::{AnalysisResult, TemplateInstantiation},
    build_info::BuildInfo,
};

/// Reports a failure to write or read a report.
#[derive(Debug, Error)]
pub enum ReportError {
    /// Indicates an I/O error.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Indicates a message-pack encoding error.
    #[error("Encode error: {0}")]
    Encode(#[from] rmp_serde::encode::Error),
    /// Indicates a message-pack decoding error.
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

/// Stores a complete build-time analysis report.
#[derive(Debug, Serialize, Deserialize)]
pub struct Report {
    /// String table backing the `u32` keys in `analysis`.
    pub strings: Vec<String>,
    /// Stores information about the build.
    pub build_info: BuildInfo,
    /// Stores the per-unit analysis results.
    pub analysis: AnalysisResult,
}

impl Report {
    /// Creates a new report from the given data.
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

    /// Reduces the report size. It keeps the largest templates and the
    /// largest headers per unit, caps the inclusion chains, and then drops
    /// strings that nothing references.
    ///
    /// `max_per_unit` limits the headers and templates kept for each unit.
    /// `max_templates` limits the templates kept for the whole build.
    pub fn slim(&mut self, max_per_unit: usize, max_templates: usize) {
        prune_analysis(&mut self.analysis, max_per_unit, max_templates);
        self.compact_strings();
    }

    /// Removes strings that the analysis does not reference.
    fn compact_strings(&mut self) {
        let mut used = vec![false; self.strings.len()];
        collect_used_strings(&self.analysis, &mut used);

        let mut remap = vec![u32::MAX; self.strings.len()];
        let mut strings = Vec::new();
        for (index, text) in self.strings.iter().enumerate() {
            if used[index] {
                remap[index] = strings.len() as u32;
                strings.push(text.clone());
            }
        }
        remap_analysis(&mut self.analysis, &remap);
        self.strings = strings;
    }
}

/// The number of inclusion chains kept for each header.
const MAX_CHAINS: usize = 8;

/// Keeps the largest `keep` entries of a map, ranked by `score`.
fn keep_largest<K, V>(map: &mut HashMap<K, V>, keep: usize, score: impl Fn(&V) -> i64)
where
    K: Eq + std::hash::Hash + Copy,
{
    if keep == 0 || map.len() <= keep {
        return;
    }
    let mut entries: Vec<(K, i64)> = map.iter().map(|(k, v)| (*k, score(v))).collect();
    entries.sort_by_key(|e| std::cmp::Reverse(e.1));
    let keep_keys: HashSet<K> = entries.into_iter().take(keep).map(|(k, _)| k).collect();
    map.retain(|k, _| keep_keys.contains(k));
}

/// Keeps the largest `keep` chains of a map, ranked by `score`.
fn keep_largest_chains(
    map: &mut BTreeMap<Vec<StringId>, i64>,
    keep: usize,
    score: impl Fn(i64) -> i64,
) {
    if keep == 0 || map.len() <= keep {
        return;
    }
    let mut entries: Vec<(Vec<StringId>, i64)> =
        map.iter().map(|(k, v)| (k.clone(), score(*v))).collect();
    entries.sort_by_key(|e| std::cmp::Reverse(e.1));
    let keep_keys: HashSet<Vec<StringId>> =
        entries.into_iter().take(keep).map(|(k, _)| k).collect();
    map.retain(|k, _| keep_keys.contains(k));
}

/// Prunes the per-unit maps and the build-wide template maps.
fn prune_analysis(analysis: &mut AnalysisResult, max_per_unit: usize, max_templates: usize) {
    for unit in analysis.unit_analysis.values_mut() {
        // Keep the slowest headers.
        keep_largest(&mut unit.header_parse_duration, max_per_unit, |v| *v);
        let kept_headers: HashSet<StringId> = unit.header_parse_duration.keys().copied().collect();
        unit.header_inclusion_paths
            .retain(|header, _| kept_headers.contains(header));
        for chains in unit.header_inclusion_paths.values_mut() {
            keep_largest_chains(chains, MAX_CHAINS, |v| v);
        }
        unit.header_usage
            .retain(|header, _| kept_headers.contains(header));

        // Keep the largest template instantiations.
        keep_largest(
            &mut unit.function_template_instantiations,
            max_per_unit,
            |t| t.total_duration,
        );
        keep_largest(&mut unit.class_template_instantiations, max_per_unit, |t| {
            t.total_duration
        });
    }

    keep_largest(
        &mut analysis.function_template_instantiations,
        max_templates,
        |t| t.total_duration,
    );
    keep_largest(
        &mut analysis.class_template_instantiations,
        max_templates,
        |t| t.total_duration,
    );

    for header in analysis.header_analysis.values_mut() {
        keep_largest_chains(&mut header.inclusion_paths, MAX_CHAINS, |v| v);
    }
}

/// Marks every string id the analysis references.
fn collect_used_strings(analysis: &AnalysisResult, used: &mut [bool]) {
    let mark = |used: &mut [bool], id: StringId| {
        if let Some(slot) = used.get_mut(id.index() as usize) {
            *slot = true;
        }
    };
    for (unit_id, unit) in &analysis.unit_analysis {
        mark(used, *unit_id);
        for header in unit.header_parse_duration.keys() {
            mark(used, *header);
        }
        for (header, chains) in &unit.header_inclusion_paths {
            mark(used, *header);
            for chain in chains.keys() {
                for part in chain {
                    mark(used, *part);
                }
            }
        }
        for header in unit.header_usage.keys() {
            mark(used, *header);
        }
        for template in unit.function_template_instantiations.keys() {
            mark(used, *template);
        }
        for template in unit.class_template_instantiations.keys() {
            mark(used, *template);
        }
    }
    for (header, detail) in &analysis.header_analysis {
        mark(used, *header);
        for chain in detail.inclusion_paths.keys() {
            for part in chain {
                mark(used, *part);
            }
        }
    }
    for template in analysis.function_template_instantiations.keys() {
        mark(used, *template);
    }
    for template in analysis.class_template_instantiations.keys() {
        mark(used, *template);
    }
}

/// Rewrites every string id with its value from `remap`.
fn remap_analysis(analysis: &mut AnalysisResult, remap: &[u32]) {
    let remap_id = |id: StringId| StringId::from(remap[id.index() as usize]);
    let remap_map = |map: HashMap<StringId, TemplateInstantiation>| {
        map.into_iter().map(|(k, v)| (remap_id(k), v)).collect()
    };

    let units = std::mem::take(&mut analysis.unit_analysis);
    analysis.unit_analysis = units
        .into_iter()
        .map(|(unit_id, mut unit)| {
            unit.header_parse_duration = unit
                .header_parse_duration
                .into_iter()
                .map(|(k, v)| (remap_id(k), v))
                .collect();
            unit.header_inclusion_paths = unit
                .header_inclusion_paths
                .into_iter()
                .map(|(header, chains)| {
                    let chains = chains
                        .into_iter()
                        .map(|(chain, v)| (chain.into_iter().map(remap_id).collect(), v))
                        .collect();
                    (remap_id(header), chains)
                })
                .collect();
            unit.header_usage = unit
                .header_usage
                .into_iter()
                .map(|(k, v)| (remap_id(k), v))
                .collect();
            unit.function_template_instantiations =
                remap_map(unit.function_template_instantiations);
            unit.class_template_instantiations = remap_map(unit.class_template_instantiations);
            (remap_id(unit_id), unit)
        })
        .collect();

    let headers = std::mem::take(&mut analysis.header_analysis);
    analysis.header_analysis = headers
        .into_iter()
        .map(|(header, mut detail)| {
            detail.inclusion_paths = detail
                .inclusion_paths
                .into_iter()
                .map(|(chain, v)| (chain.into_iter().map(remap_id).collect(), v))
                .collect();
            (remap_id(header), detail)
        })
        .collect();

    analysis.function_template_instantiations = remap_map(std::mem::take(
        &mut analysis.function_template_instantiations,
    ));
    analysis.class_template_instantiations =
        remap_map(std::mem::take(&mut analysis.class_template_instantiations));
}

/// Writes the report to the file at `path`.
///
/// Structs are encoded positionally (as MessagePack arrays) rather than as
/// maps with field names. A large report repeats the same struct millions of
/// times, so the field names dominated the payload when using
/// `with_struct_map()`; positional encoding removes them entirely. The field
/// order in each struct is therefore part of the on-disk format.
pub fn write_report(report: &Report, path: &Path) -> Result<(), ReportError> {
    let file = File::create(path)?;
    let writer = BufWriter::new(file);
    let mut gz = GzEncoder::new(writer, Compression::default());
    let mut serializer = rmp_serde::Serializer::new(&mut gz);
    report.serialize(&mut serializer)?;
    gz.finish()?;
    Ok(())
}

/// Reads a report from the file at `path`.
pub fn read_report(path: &Path) -> Result<Report, ReportError> {
    let file = File::open(path)?;
    let reader = BufReader::new(file);
    let gz = GzDecoder::new(reader);
    let report: Report = rmp_serde::from_read(gz)?;
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
            header_usage: HashMap::new(),
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
                header_usage: HashMap::new(),
                function_template_instantiations: HashMap::new(),
                class_template_instantiations: HashMap::new(),
            },
        );
        Report::new(strings, BuildInfo::default(), AnalysisResult::from(units))
    }

    fn encode(report: &Report) -> Vec<u8> {
        let mut buf = Vec::new();
        let mut ser = rmp_serde::Serializer::new(&mut buf);
        report.serialize(&mut ser).unwrap();
        buf
    }

    #[test]
    fn serialization_is_byte_stable() {
        let report = sample_report();
        assert_eq!(encode(&report), encode(&report));
    }

    #[test]
    fn structs_are_encoded_without_field_names() {
        let bytes = encode(&sample_report());
        // Positional encoding must not repeat struct field names.
        for name in [
            b"total_duration".as_slice(),
            b"num_instantiations",
            b"parse_duration",
            b"frontend_duration",
        ] {
            assert!(
                !bytes.windows(name.len()).any(|w| w == name),
                "encoded report contains field name {:?}",
                std::str::from_utf8(name).unwrap()
            );
        }
    }

    #[test]
    fn written_report_is_gzip_compressed_and_smaller() {
        // Use a report large enough that gzip beats its own framing overhead.
        let names: Vec<String> = (0..500)
            .map(|i| format!("translation_unit_{i:04}.cpp"))
            .collect();
        let durations: Vec<(&str, i64)> = names.iter().map(|n| (n.as_str(), 1000)).collect();
        let report = report_with_unit_durations(&durations);
        let path = std::env::temp_dir().join("gzip_check.bin");
        write_report(&report, &path).unwrap();
        let file_bytes = std::fs::read(&path).unwrap();
        std::fs::remove_file(path).ok();

        // gzip magic bytes (0x1f 0x8b)
        assert_eq!(&file_bytes[..2], &[0x1f, 0x8b]);
        // Compression must shrink the payload
        assert!(file_bytes.len() < encode(&report).len());
    }

    #[test]
    fn gzip_output_is_deterministic() {
        let report = sample_report();
        let path = std::env::temp_dir().join("gzip_det1.bin");
        let path2 = std::env::temp_dir().join("gzip_det2.bin");
        write_report(&report, &path).unwrap();
        write_report(&report, &path2).unwrap();
        let a = std::fs::read(&path).unwrap();
        let b = std::fs::read(&path2).unwrap();
        std::fs::remove_file(path).ok();
        std::fs::remove_file(path2).ok();
        assert_eq!(a, b);
    }

    #[test]
    fn write_then_read_round_trips() {
        let report = sample_report();
        let path = std::env::temp_dir().join("roundtrip.bin");
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
                    header_usage: HashMap::new(),
                    function_template_instantiations: HashMap::new(),
                    class_template_instantiations: HashMap::new(),
                },
            );
        }
        Report::new(strings, BuildInfo::default(), AnalysisResult::from(units))
    }

    #[test]
    fn slim_prunes_data_and_keeps_ids_valid() {
        // The string table must be sorted for `id_of`.
        let mut strings: Vec<String> = (0..30).map(|i| format!("s{i:02}")).collect();
        strings.sort();
        let id_of = |s: &str| {
            StringId::from(strings.binary_search_by(|x| x.as_str().cmp(s)).unwrap() as u32)
        };

        let mut unit = UnitAnalysis {
            total_duration: 100,
            frontend_duration: 0,
            backend_duration: 0,
            template_instantiation_duration: 0,
            header_parse_duration: HashMap::new(),
            header_inclusion_paths: BTreeMap::new(),
            header_usage: HashMap::new(),
            function_template_instantiations: HashMap::new(),
            class_template_instantiations: HashMap::new(),
        };
        for i in 0..20 {
            unit.header_parse_duration
                .insert(id_of(&format!("s{i:02}")), i as i64);
            unit.function_template_instantiations.insert(
                id_of(&format!("s{i:02}")),
                TemplateInstantiation::new(i as i64),
            );
        }
        let units = HashMap::from([(id_of("s00"), unit)]);
        let mut report = Report::new(
            strings.clone(),
            BuildInfo::default(),
            AnalysisResult::from(units),
        );

        report.slim(5, 3);

        let unit = report.analysis.unit_analysis.values().next().unwrap();
        assert_eq!(unit.header_parse_duration.len(), 5);
        assert_eq!(unit.function_template_instantiations.len(), 5);
        assert!(report.analysis.function_template_instantiations.len() <= 3);

        // Every remaining id must resolve inside the string table.
        for id in unit.header_parse_duration.keys() {
            assert!(!report.resolve(*id).is_empty());
        }

        // The pruned report must still write and read back.
        let path = std::env::temp_dir().join("slim_roundtrip.bin");
        write_report(&report, &path).unwrap();
        let read = read_report(&path).unwrap();
        assert_eq!(read.analysis.unit_analysis.len(), 1);
        std::fs::remove_file(path).ok();
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

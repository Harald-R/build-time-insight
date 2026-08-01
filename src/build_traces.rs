use rayon::prelude::*;
use serde::Deserialize;
use serde_json::Value;
use std::{fs::File, io::BufReader};

#[derive(Deserialize, Debug)]
struct TraceEventRaw {
    name: String,        // Name of the event
    cat: Option<String>, // Event categories
    ph: String,          // Event type (phase)
    pid: i64,            // Process ID
    tid: i64,            // Thread ID
    ts: i64,             // Event timestamp (in us)
    dur: Option<i64>,    // Event duration (in us)
    args: Option<Value>, // Extra arguments
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
struct TraceEventFile {
    trace_events: Vec<TraceEventRaw>,
    beginning_of_time: i64,
}

fn parse_traces(build_dir: &str) -> Vec<TraceEventFile> {
    let pattern = build_dir.to_owned() + "/**/*.cpp.json";
    let paths: Vec<_> = match glob::glob(&pattern) {
        Ok(paths) => paths.filter_map(|e| e.ok()).collect(),
        Err(e) => {
            eprintln!("Glob pattern error: {}", e);
            return vec![];
        }
    };
    paths
        .into_par_iter()
        .filter_map(|path| File::open(path).ok())
        .map(BufReader::new)
        .filter_map(|reader| serde_json::from_reader(reader).ok())
        .collect()
}

pub fn collect_build_traces(build_dir: &str) -> Result<(), String> {
    let trace_events = parse_traces(build_dir);
    println!("{:#?}", trace_events);
    Ok(())
}

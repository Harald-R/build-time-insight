use rayon::prelude::*;
use serde::Deserialize;
use serde_json::Value;
use std::{collections::HashMap, fs::File, io::BufReader, path::PathBuf};

const OPT_MODULE: &str = "OptModule";
const EXECUTE_COMPILER: &str = "ExecuteCompiler";
const FRONTEND: &str = "Frontend";
const BACKEND: &str = "Backend";

#[derive(Debug)]
struct Trace {
    source_file: String,
    total_duration: i64,
    frontend_duration: i64,
    backend_duration: i64,
}

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
struct TraceEventFileRaw {
    trace_events: Vec<TraceEventRaw>,
    beginning_of_time: i64,
}

impl TraceEventFileRaw {
    pub fn extract_trace(self) -> Trace {
        let mut trace = Trace {
            source_file: String::new(),
            total_duration: 0,
            frontend_duration: 0,
            backend_duration: 0,
        };
        for event in self.trace_events {
            if event.name == OPT_MODULE {
                trace.source_file = match event.args {
                    Some(args) => {
                        if let Some(detail) = args.get("detail") {
                            detail.to_string().trim_matches('"').to_string()
                        } else {
                            continue;
                        }
                    }
                    None => continue,
                };
            } else if event.name == EXECUTE_COMPILER {
                trace.total_duration += event.dur.unwrap_or(0);
            } else if event.name == FRONTEND {
                trace.frontend_duration += event.dur.unwrap_or(0);
            } else if event.name == BACKEND {
                trace.backend_duration += event.dur.unwrap_or(0);
            }
        }
        trace
    }
}

fn parse_traces(build_dir: &str) -> HashMap<String, TraceEventFileRaw> {
    let pattern = build_dir.to_owned() + "/**/*.cpp.json";
    let paths: Vec<PathBuf> = match glob::glob(&pattern) {
        Ok(paths) => paths.filter_map(|e| e.ok()).collect(),
        Err(e) => {
            eprintln!("Glob pattern error: {}", e);
            return HashMap::new();
        }
    };
    paths
        .into_par_iter()
        .filter_map(|path| {
            let path_str = path.to_str()?.to_string();
            let file = File::open(&path).ok()?;
            let reader = BufReader::new(file);
            let trace = serde_json::from_reader(reader).ok()?;
            Some((path_str, trace))
        })
        .collect()
}

pub fn collect_build_traces(build_dir: &str) -> Result<(), String> {
    let trace_event_files = parse_traces(build_dir);

    let traces: HashMap<String, Trace> = trace_event_files
        .into_par_iter()
        .filter_map(|trace_event_file| {
            Some((trace_event_file.0, trace_event_file.1.extract_trace()))
        })
        .collect();
    println!("Traces {:#?}", traces);

    Ok(())
}

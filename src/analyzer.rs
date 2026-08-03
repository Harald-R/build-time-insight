use rayon::iter::{IntoParallelRefIterator, ParallelIterator};

use crate::build_traces::FileTrace;
use crate::raw_trace::*;

pub fn analyze(traces: Vec<FileTrace>) {
    for trace in &traces {
        trace.trace_tree.print_tree(0, 0);
    }
    traces.par_iter().for_each(|trace| {
        let mut total_duration: i64 = 0;
        let mut frontend_duration: i64 = 0;
        let mut backend_duration: i64 = 0;
        trace.trace_tree.nodes.iter().for_each(|node| {
            if node.trace.name == EXECUTE_COMPILER {
                let duration = node.trace.dur.unwrap_or(0);
                total_duration += duration;
            } else if node.trace.name == FRONTEND {
                let duration = node.trace.dur.unwrap_or(0);
                frontend_duration += duration;
            } else if node.trace.name == BACKEND {
                let duration = node.trace.dur.unwrap_or(0);
                backend_duration += duration;
            }
        });
        println!(
            "total_duration {}, frontend_duration {}, backend_duration {}",
            total_duration, frontend_duration, backend_duration
        );
    });
}

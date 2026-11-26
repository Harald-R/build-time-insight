//! Defines the low-level trace event types decoded from clang output.

use serde::Deserialize;

pub const TOTAL_EXECUTE_COMPILER: &str = "Total ExecuteCompiler";
pub const TOTAL_FRONTEND: &str = "Total Frontend";
pub const TOTAL_BACKEND: &str = "Total Backend";
pub const PARSE_DECL_OR_FN_DEF: &str = "ParseDeclarationOrFunctionDefinition";
pub const INSTANTIATE_FUNCTION: &str = "InstantiateFunction";
pub const INSTANTIATE_CLASS: &str = "InstantiateClass";
pub const PERFORM_PENDING_INSTANTIATIONS: &str = "PerformPendingInstantiations";
pub const OPT_MODULE: &str = "OptModule";
pub const SOURCE: &str = "Source";

/// The subset of an event's `args` object that the tool consumes. Unknown
/// keys are ignored, avoiding the cost of materializing the full JSON value.
#[derive(Deserialize, Debug, Clone, Default)]
pub struct TraceArgs {
    /// Detail text of the event.
    pub detail: Option<String>,
}

/// A trace event from the JSON trace file.
#[derive(Deserialize, Debug, Clone)]
pub struct TraceEvent {
    /// Name of the event.
    pub name: String,
    /// Categories of the event.
    pub cat: Option<String>,
    /// Phase type of the event.
    pub ph: String,
    /// Process ID of the event.
    pub pid: i64,
    /// Thread ID of the event.
    pub tid: i64,
    /// ID of the event, for async events.
    pub id: Option<i64>,
    /// Timestamp of the event, in microseconds.
    pub ts: i64,
    /// Duration of the event, in microseconds.
    pub dur: Option<i64>,
    #[serde(default)]
    /// Extra arguments of the event.
    pub args: Option<TraceArgs>,
}

impl TraceEvent {
    #[inline]
    /// Check whether this event encloses another event.
    pub fn encloses(&self, other: &Self) -> bool {
        let self_dur = self.dur.unwrap_or(0);
        let other_dur = other.dur.unwrap_or(0);
        self.ts <= other.ts && (self.ts + self_dur) >= (other.ts + other_dur)
    }
}

/// A trace event file from the JSON trace.
#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TraceEventFile {
    /// Trace events in the file.
    pub trace_events: Vec<TraceEvent>,
    /// Beginning time of the trace, in microseconds.
    #[serde(default)]
    pub beginning_of_time: i64,
}

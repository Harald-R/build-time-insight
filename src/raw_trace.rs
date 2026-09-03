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
    pub detail: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct TraceEvent {
    pub name: String,        // Name of the event
    pub cat: Option<String>, // Event categories
    pub ph: String,          // Event type (phase)
    pub pid: i64,            // Process ID
    pub tid: i64,            // Thread ID
    pub id: Option<i64>,     // ID (used for async events)
    pub ts: i64,             // Event timestamp (in us)
    pub dur: Option<i64>,    // Event duration (in us)
    #[serde(default)]
    pub args: Option<TraceArgs>, // Extra arguments (only the fields we use)
}

impl TraceEvent {
    #[inline]
    pub fn encloses(&self, other: &Self) -> bool {
        let self_dur = self.dur.unwrap_or(0);
        let other_dur = other.dur.unwrap_or(0);
        self.ts <= other.ts && (self.ts + self_dur) >= (other.ts + other_dur)
    }
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TraceEventFile {
    pub trace_events: Vec<TraceEvent>,
    pub beginning_of_time: i64,
}

use serde::Deserialize;
use serde_json::Value;

pub const TOTAL_EXECUTE_COMPILER: &str = "Total ExecuteCompiler";
pub const TOTAL_FRONTEND: &str = "Total Frontend";
pub const TOTAL_BACKEND: &str = "Total Backend";
pub const PARSE_DECL_OR_FN_DEF: &str = "ParseDeclarationOrFunctionDefinition";
pub const OPT_MODULE: &str = "OptModule";
pub const SOURCE: &str = "Source";
pub const DETAIL: &str = "detail";

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
    pub args: Option<Value>, // Extra arguments
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

use serde::Deserialize;
use serde_json::Value;

pub const EXECUTE_COMPILER: &str = "ExecuteCompiler";
pub const FRONTEND: &str = "Frontend";
pub const BACKEND: &str = "Backend";
pub const PARSE_DECL_OR_FN_DEF: &str = "ParseDeclarationOrFunctionDefinition";
pub const PARSE_FN_DEF: &str = "ParseFunctionDefinition";
pub const PARSE_CLASS: &str = "ParseClass";
pub const INSTANTIATE_CLASS: &str = "InstantiateClass";
pub const INSTANTIATE_FUNCTION: &str = "InstantiateFunction";
pub const CODE_GEN_FUNCTION: &str = "CodeGen Function";
pub const PERFORM_PENDING_INSTANTIATIONS: &str = "PerformPendingInstantiations";
pub const OPTIMIZER: &str = "Optimizer";
pub const CODE_GEN_PASSES: &str = "CodeGenPasses";
pub const OPT_MODULE: &str = "OptModule";
pub const SOURCE: &str = "Source";

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
    pub fn end_time(&self) -> Option<i64> {
        Some(self.ts + self.dur?)
    }

    pub fn encloses(&self, other: &TraceEvent) -> bool {
        self.ts <= other.ts && self.end_time() >= other.end_time()
    }
}

#[derive(Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TraceEventFile {
    pub trace_events: Vec<TraceEvent>,
    pub beginning_of_time: i64,
}

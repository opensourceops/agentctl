#![no_main]

use agentctl_protocols::{AgentCard, McpTool};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = serde_json::from_slice::<AgentCard>(data);
    let _ = serde_json::from_slice::<McpTool>(data);
});

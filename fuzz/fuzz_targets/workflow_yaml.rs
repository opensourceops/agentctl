#![no_main]

use agentctl_core::parse_workflow;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(source) = std::str::from_utf8(data) {
        let _ = parse_workflow(source, "fuzz.yaml");
    }
});

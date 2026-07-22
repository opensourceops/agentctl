#![no_main]

use agentctl_core::template::{EvalContext, render, validate_expression};
use libfuzzer_sys::fuzz_target;
use serde_json::Value;

fuzz_target!(|data: &[u8]| {
    if let Ok(template) = std::str::from_utf8(data) {
        let _ = validate_expression(template);
        let _ = render(&Value::String(template.to_owned()), &EvalContext::default());
    }
});

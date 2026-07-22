#![no_main]

use agentctl_core::tool::ToolContract;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(value) = serde_json::from_slice(data) else {
        return;
    };
    if let Ok(contract) = serde_json::from_value::<ToolContract>(value) {
        let _ = contract.validate_input(&serde_json::Value::Null);
        let _ = contract.validate_output(&serde_json::Value::Null);
    }
});

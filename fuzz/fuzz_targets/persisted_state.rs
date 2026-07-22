#![no_main]

use agentctl_core::effect::EffectRecord;
use agentctl_store::{RunRecord, TaskRecord};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = serde_json::from_slice::<EffectRecord>(data);
    let _ = serde_json::from_slice::<RunRecord>(data);
    let _ = serde_json::from_slice::<TaskRecord>(data);
});

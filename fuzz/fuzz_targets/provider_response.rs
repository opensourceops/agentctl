#![no_main]

use agentctl_core::provider::ProviderResponse;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = serde_json::from_slice::<ProviderResponse>(data);
});

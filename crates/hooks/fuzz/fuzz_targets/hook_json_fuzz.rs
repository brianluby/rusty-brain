#![no_main]

use libfuzzer_sys::fuzz_target;
use types::HookInput;

fuzz_target!(|data: &[u8]| {
    // Try to parse as HookInput — must never panic
    let _ = serde_json::from_slice::<HookInput>(data);

    // Also try as generic JSON Value
    let _ = serde_json::from_slice::<serde_json::Value>(data);

    // Try string-based parsing as well
    if let Ok(s) = std::str::from_utf8(data) {
        let _ = serde_json::from_str::<HookInput>(s);
        let _ = serde_json::from_str::<serde_json::Value>(s);
    }
});

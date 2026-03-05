#![no_main]

use compression::{CompressionConfig, compress};
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let config = CompressionConfig::default();
    if let Ok(input) = std::str::from_utf8(data) {
        let tools = [
            "Read", "Write", "Bash", "Glob", "Grep", "LS", "Edit", "Unknown",
        ];
        for tool in &tools {
            let _ = compress(&config, tool, input, None);
        }
    }
});

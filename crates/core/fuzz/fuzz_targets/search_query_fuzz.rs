#![no_main]

use std::sync::OnceLock;

use libfuzzer_sys::fuzz_target;

static MIND: OnceLock<(tempfile::TempDir, rusty_brain_core::mind::Mind)> = OnceLock::new();

fn get_mind() -> &'static rusty_brain_core::mind::Mind {
    &MIND
        .get_or_init(|| {
            let dir = tempfile::tempdir().expect("failed to create temp dir for fuzz mind");
            let config = types::MindConfig {
                memory_path: dir.path().join("fuzz.mv2"),
                ..types::MindConfig::default()
            };
            let mind =
                rusty_brain_core::mind::Mind::open(config).expect("failed to open fuzz mind");
            (dir, mind)
        })
        .1
}

fuzz_target!(|data: &[u8]| {
    if let Ok(query) = std::str::from_utf8(data) {
        // The mind must never panic on arbitrary query strings
        let _ = get_mind().search(query, Some(5));
    }
});

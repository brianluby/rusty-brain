use criterion::{Criterion, criterion_group, criterion_main};
use types::hooks::HookInput;

fn make_session_start_input(cwd: &str) -> HookInput {
    serde_json::from_value(serde_json::json!({
        "session_id": "bench-001",
        "transcript_path": "/tmp/bench.jsonl",
        "cwd": cwd,
        "permission_mode": "default",
        "hook_event_name": "SessionStart",
        "platform": "claude"
    }))
    .unwrap()
}

fn make_post_tool_use_input(cwd: &str) -> HookInput {
    serde_json::from_value(serde_json::json!({
        "session_id": "bench-001",
        "transcript_path": "/tmp/bench.jsonl",
        "cwd": cwd,
        "permission_mode": "default",
        "hook_event_name": "PostToolUse",
        "tool_name": "Read",
        "tool_input": {"file_path": "/tmp/test.rs"},
        "tool_response": "fn main() { println!(\"hello\"); }"
    }))
    .unwrap()
}

fn bench_session_start(c: &mut Criterion) {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_str().unwrap().to_string();
    let input = make_session_start_input(&cwd);

    c.bench_function("session_start", |b| {
        b.iter(|| {
            let _ = hooks::session_start::handle_session_start(&input);
        });
    });
}

fn bench_post_tool_use(c: &mut Criterion) {
    let dir = tempfile::tempdir().unwrap();
    let cwd = dir.path().to_str().unwrap().to_string();
    let input = make_post_tool_use_input(&cwd);

    c.bench_function("post_tool_use", |b| {
        b.iter(|| {
            let _ = hooks::post_tool_use::handle_post_tool_use(&input);
        });
    });
}

criterion_group!(benches, bench_session_start, bench_post_tool_use);
criterion_main!(benches);

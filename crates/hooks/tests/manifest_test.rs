use hooks::manifest::generate_manifest;

#[test]
fn generated_json_contains_all_event_types() {
    let manifest = generate_manifest("rusty-brain");
    let parsed: serde_json::Value =
        serde_json::from_str(&manifest).expect("manifest must be valid JSON");

    let hooks = parsed.get("hooks").expect("must have 'hooks' key");
    assert!(
        hooks.get("SessionStart").is_some(),
        "must contain SessionStart"
    );
    assert!(
        hooks.get("PostToolUse").is_some(),
        "must contain PostToolUse"
    );
    assert!(hooks.get("Stop").is_some(), "must contain Stop");
    assert!(
        hooks.get("Notification").is_some(),
        "must contain Notification"
    );
}

#[test]
fn each_entry_has_type_command_with_correct_command_string() {
    let manifest = generate_manifest("rusty-brain");
    let parsed: serde_json::Value = serde_json::from_str(&manifest).unwrap();
    let hooks = parsed.get("hooks").unwrap();

    let expected = [
        ("SessionStart", "rusty-brain session-start"),
        ("PostToolUse", "rusty-brain post-tool-use"),
        ("Stop", "rusty-brain stop"),
        ("Notification", "rusty-brain smart-install"),
    ];

    for (event, cmd) in &expected {
        let entries = hooks.get(*event).unwrap().as_array().unwrap();
        assert_eq!(entries.len(), 1, "{event} should have exactly one entry");
        let entry = &entries[0];
        assert_eq!(entry.get("type").unwrap().as_str().unwrap(), "command");
        assert_eq!(entry.get("command").unwrap().as_str().unwrap(), *cmd);
    }
}

#[test]
fn json_is_valid_and_parseable() {
    let manifest = generate_manifest("rusty-brain");
    let parsed: Result<serde_json::Value, _> = serde_json::from_str(&manifest);
    assert!(parsed.is_ok(), "manifest must produce valid JSON");
}

#[test]
fn binary_name_is_configurable() {
    let manifest = generate_manifest("my-custom-binary");
    let parsed: serde_json::Value = serde_json::from_str(&manifest).unwrap();
    let hooks = parsed.get("hooks").unwrap();

    let session_start = hooks.get("SessionStart").unwrap().as_array().unwrap();
    let cmd = session_start[0].get("command").unwrap().as_str().unwrap();
    assert!(
        cmd.starts_with("my-custom-binary"),
        "should use custom binary name"
    );
}

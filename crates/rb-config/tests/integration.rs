//! Integration tests for `rb_config`: public API surface exercised from
//! outside the crate (no `#[cfg(test)]` access). Each test covers a behaviour
//! that the inline unit tests do not already exercise.
//!
//! Tests that mutate process-global env vars are serialised through `ENV_LOCK`.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::Mutex;

use rb_config::namespace::{
    accept_override, default_pins_path, explicit_namespace_from_env, project_from_frontmatter,
    resolve_namespace_in, NamespacePins, NamespaceResolution, RepoConfigDivergenceKind,
    UnpinnedOverride, REPO_CONFIG_FILE,
};
use rb_config::{
    current_user, default_db_path, default_socket_path, resolve_db_path, resolve_socket_path,
    ACCEPT_MODEL_CHANGE_ENV, DB_ENV, FORWARD_ENV, IDLE_TIMEOUT_ENV, JOBS_CONFIG_ENV, NAMESPACE_ENV,
    SOCKET_ENV,
};
use rb_types::Namespace;
use tempfile::TempDir;

// Serialise all tests that touch process-global env vars.
static ENV_LOCK: Mutex<()> = Mutex::new(());

// ---------------------------------------------------------------------------
// Constant surface: names must never silently change (would break the protocol
// between daemon, CLI, and hooks).
// ---------------------------------------------------------------------------

#[test]
fn socket_env_constant_has_expected_name() {
    assert_eq!(SOCKET_ENV, "RUSTY_BRAIN_SOCKET");
}

#[test]
fn namespace_env_constant_has_expected_name() {
    assert_eq!(NAMESPACE_ENV, "RUSTY_BRAIN_NAMESPACE");
}

#[test]
fn db_env_constant_has_expected_name() {
    assert_eq!(DB_ENV, "RUSTY_BRAIN_DB");
}

#[test]
fn jobs_config_env_constant_has_expected_name() {
    assert_eq!(JOBS_CONFIG_ENV, "RB_JOBS_CONFIG");
}

#[test]
fn accept_model_change_env_constant_has_expected_name() {
    assert_eq!(ACCEPT_MODEL_CHANGE_ENV, "RB_ACCEPT_MODEL_CHANGE");
}

#[test]
fn idle_timeout_env_constant_has_expected_name() {
    assert_eq!(IDLE_TIMEOUT_ENV, "RUSTY_BRAIN_IDLE_TIMEOUT_SECS");
}

// C1 contract: FORWARD_ENV is secrets + identity + XDG/HOME ONLY — knobs like
// the model-swap opt-in must NOT creep back in (they reach auto-started
// daemons via the config file, or via the frozen LEGACY_KNOB_ENV compat list,
// which spawn_forward_env still forwards when explicitly set).
#[test]
fn forward_env_excludes_knobs_but_spawn_forwarding_keeps_legacy_compat() {
    assert!(
        !FORWARD_ENV.contains(&ACCEPT_MODEL_CHANGE_ENV),
        "FORWARD_ENV must not regrow the {ACCEPT_MODEL_CHANGE_ENV} knob (C1)"
    );
    assert!(
        rb_config::LEGACY_KNOB_ENV.contains(&ACCEPT_MODEL_CHANGE_ENV),
        "{ACCEPT_MODEL_CHANGE_ENV} must stay supported via the frozen legacy list"
    );
    assert!(
        rb_config::spawn_forward_env().any(|v| v == ACCEPT_MODEL_CHANGE_ENV),
        "spawners must still forward {ACCEPT_MODEL_CHANGE_ENV} for compatibility"
    );
}

// ---------------------------------------------------------------------------
// current_user: USER -> LOGNAME -> None priority chain
// ---------------------------------------------------------------------------

/// LOGNAME is the fallback when USER is absent (W0.5 provenance).
#[test]
fn current_user_falls_back_to_logname_when_user_is_unset() {
    let _lock = ENV_LOCK.lock().unwrap();
    let prev_user = std::env::var_os("USER");
    let prev_logname = std::env::var_os("LOGNAME");
    std::env::remove_var("USER");
    std::env::set_var("LOGNAME", "logname-fallback-user");
    let result = current_user();
    // Restore env.
    match prev_user {
        Some(v) => std::env::set_var("USER", v),
        None => std::env::remove_var("USER"),
    }
    match prev_logname {
        Some(v) => std::env::set_var("LOGNAME", v),
        None => std::env::remove_var("LOGNAME"),
    }
    assert_eq!(result.as_deref(), Some("logname-fallback-user"));
}

/// When both USER and LOGNAME are absent the function degrades to None
/// — provenance is unavailable in that environment, not an error.
#[test]
fn current_user_is_none_when_both_user_and_logname_are_unset() {
    let _lock = ENV_LOCK.lock().unwrap();
    let prev_user = std::env::var_os("USER");
    let prev_logname = std::env::var_os("LOGNAME");
    std::env::remove_var("USER");
    std::env::remove_var("LOGNAME");
    let result = current_user();
    match prev_user {
        Some(v) => std::env::set_var("USER", v),
        None => std::env::remove_var("USER"),
    }
    match prev_logname {
        Some(v) => std::env::set_var("LOGNAME", v),
        None => std::env::remove_var("LOGNAME"),
    }
    assert!(
        result.is_none(),
        "current_user must be None when USER and LOGNAME are both unset"
    );
}

/// An empty string USER is not treated as a valid user; falls back to LOGNAME.
#[test]
fn current_user_ignores_empty_user_and_uses_logname() {
    let _lock = ENV_LOCK.lock().unwrap();
    let prev_user = std::env::var_os("USER");
    let prev_logname = std::env::var_os("LOGNAME");
    std::env::set_var("USER", "");
    std::env::set_var("LOGNAME", "non-empty-logname");
    let result = current_user();
    match prev_user {
        Some(v) => std::env::set_var("USER", v),
        None => std::env::remove_var("USER"),
    }
    match prev_logname {
        Some(v) => std::env::set_var("LOGNAME", v),
        None => std::env::remove_var("LOGNAME"),
    }
    assert_eq!(
        result.as_deref(),
        Some("non-empty-logname"),
        "empty USER must be skipped in favour of LOGNAME"
    );
}

// ---------------------------------------------------------------------------
// default_pins_path: structural check (no env mutation needed).
// ---------------------------------------------------------------------------

#[test]
fn default_pins_path_ends_with_namespace_pins_toml_in_rusty_brain_dir() {
    let _env_guard = ENV_LOCK.lock().unwrap();
    let p = default_pins_path().unwrap();
    assert_eq!(
        p.file_name().and_then(|s| s.to_str()),
        Some("namespace-pins.toml"),
        "pins file must be named namespace-pins.toml: {p:?}"
    );
    assert_eq!(
        p.parent()
            .and_then(|d| d.file_name())
            .and_then(|s| s.to_str()),
        Some("rusty-brain"),
        "pins file must live in a rusty-brain directory: {p:?}"
    );
}

// ---------------------------------------------------------------------------
// default_socket_path / default_db_path: structural invariants (public API).
// ---------------------------------------------------------------------------

#[test]
fn default_db_path_ends_with_rusty_brain_memory_db() {
    let _env_guard = ENV_LOCK.lock().unwrap();
    let p = default_db_path().unwrap();
    assert_eq!(p.file_name().and_then(|s| s.to_str()), Some("memory.db"));
    assert_eq!(
        p.parent()
            .and_then(|d| d.file_name())
            .and_then(|s| s.to_str()),
        Some("rusty-brain"),
        "db must live in a rusty-brain directory: {p:?}"
    );
}

#[test]
fn default_socket_path_is_named_sock() {
    let _env_guard = ENV_LOCK.lock().unwrap();
    let p = default_socket_path().unwrap();
    assert_eq!(p.file_name().and_then(|s| s.to_str()), Some("sock"));
}

// XDG_STATE_HOME overrides the state dir used for the pins file.
#[test]
fn xdg_state_home_is_honored_for_pins_path() {
    let _lock = ENV_LOCK.lock().unwrap();
    let dir = TempDir::new().unwrap();
    let prev = std::env::var_os("XDG_STATE_HOME");
    std::env::set_var("XDG_STATE_HOME", dir.path());
    let p = default_pins_path().unwrap();
    match prev {
        Some(v) => std::env::set_var("XDG_STATE_HOME", v),
        None => std::env::remove_var("XDG_STATE_HOME"),
    }
    assert!(
        p.starts_with(dir.path()),
        "pins path must live under XDG_STATE_HOME when set: {p:?}"
    );
}

// ---------------------------------------------------------------------------
// explicit_namespace_from_env: covers the env-var branch of resolution.
// ---------------------------------------------------------------------------

#[test]
fn explicit_namespace_from_env_returns_none_when_var_unset() {
    let _lock = ENV_LOCK.lock().unwrap();
    let prev = std::env::var_os(NAMESPACE_ENV);
    std::env::remove_var(NAMESPACE_ENV);
    let result = explicit_namespace_from_env();
    match prev {
        Some(v) => std::env::set_var(NAMESPACE_ENV, v),
        None => std::env::remove_var(NAMESPACE_ENV),
    }
    assert!(result.is_none());
}

#[test]
fn explicit_namespace_from_env_returns_trimmed_value() {
    let _lock = ENV_LOCK.lock().unwrap();
    let prev = std::env::var_os(NAMESPACE_ENV);
    std::env::set_var(NAMESPACE_ENV, "  my-project  ");
    let result = explicit_namespace_from_env();
    match prev {
        Some(v) => std::env::set_var(NAMESPACE_ENV, v),
        None => std::env::remove_var(NAMESPACE_ENV),
    }
    assert_eq!(result.as_deref(), Some("my-project"));
}

#[test]
fn explicit_namespace_from_env_whitespace_only_returns_none() {
    let _lock = ENV_LOCK.lock().unwrap();
    let prev = std::env::var_os(NAMESPACE_ENV);
    std::env::set_var(NAMESPACE_ENV, "   ");
    let result = explicit_namespace_from_env();
    match prev {
        Some(v) => std::env::set_var(NAMESPACE_ENV, v),
        None => std::env::remove_var(NAMESPACE_ENV),
    }
    assert!(
        result.is_none(),
        "whitespace-only RUSTY_BRAIN_NAMESPACE must not be treated as an explicit override"
    );
}

// ---------------------------------------------------------------------------
// project_from_frontmatter: edge cases beyond the inline tests.
// ---------------------------------------------------------------------------

#[test]
fn single_quoted_project_value_is_parsed() {
    let text = "---\nproject: 'my-project'\n---\n";
    assert_eq!(
        project_from_frontmatter(text),
        Some("my-project".to_string())
    );
}

#[test]
fn project_value_without_quotes_is_parsed() {
    let text = "---\nproject: bare-value\n---\n";
    assert_eq!(
        project_from_frontmatter(text),
        Some("bare-value".to_string())
    );
}

#[test]
fn unclosed_frontmatter_with_project_key_still_returns_value() {
    // Frontmatter with opening `---` but no closing fence: the parser walks
    // until EOF. If it finds a `project:` line, it returns the value.
    let text = "---\nproject: no-close\nother: x\n";
    assert_eq!(project_from_frontmatter(text), Some("no-close".to_string()));
}

#[test]
fn no_frontmatter_at_all_returns_none() {
    // Plain body text with no `---` opener: no frontmatter, no project.
    let text = "# My Project\nsome body text\n";
    assert_eq!(project_from_frontmatter(text), None);
}

#[test]
fn frontmatter_without_project_key_returns_none() {
    let text = "---\nauthor: alice\ndate: 2024-01-01\n---\nbody\n";
    assert_eq!(project_from_frontmatter(text), None);
}

#[test]
fn project_key_after_closing_fence_is_not_parsed() {
    // A `project:` line that appears AFTER the closing `---` is body text,
    // not frontmatter, and must not be mistaken for the project field.
    let text = "---\nauthor: alice\n---\nproject: body-text\n";
    assert_eq!(project_from_frontmatter(text), None);
}

// ---------------------------------------------------------------------------
// NamespacePins: insert/replace semantics.
// ---------------------------------------------------------------------------

#[test]
fn pins_insert_overwrites_existing_entry_for_same_dir() {
    let mut pins = NamespacePins::default();
    let dir = Path::new("/home/user/my-repo");
    pins.insert(dir, "first");
    pins.insert(dir, "second");
    assert_eq!(pins.pinned(dir), Some("second"));
}

#[test]
fn pins_are_per_directory_key() {
    let mut pins = NamespacePins::default();
    let a = Path::new("/home/user/repo-a");
    let b = Path::new("/home/user/repo-b");
    pins.insert(a, "ns-a");
    pins.insert(b, "ns-b");
    assert_eq!(pins.pinned(a), Some("ns-a"));
    assert_eq!(pins.pinned(b), Some("ns-b"));
    assert_eq!(pins.pinned(Path::new("/home/user/repo-c")), None);
}

#[test]
fn empty_pins_has_no_entries() {
    let pins = NamespacePins::default();
    assert_eq!(pins.pinned(Path::new("/any/path")), None);
}

#[test]
fn save_to_creates_parent_directories_automatically() {
    let tmp = TempDir::new().unwrap();
    // Several levels of directories that do not exist yet.
    let path = tmp
        .path()
        .join("a")
        .join("b")
        .join("c")
        .join("namespace-pins.toml");
    let mut pins = NamespacePins::default();
    pins.insert(Path::new("/repo"), "ns");
    pins.save_to(&path).unwrap();
    assert!(path.exists(), "save_to must create parent dirs: {path:?}");
    let loaded = NamespacePins::load_from(&path);
    assert_eq!(loaded.pinned(Path::new("/repo")), Some("ns"));
}

// ---------------------------------------------------------------------------
// accept_override: persists a pin and returns the claimed namespace.
// ---------------------------------------------------------------------------

#[test]
fn accept_override_records_pin_on_disk_and_returns_namespace() {
    let _lock = ENV_LOCK.lock().unwrap();
    let tmp = TempDir::new().unwrap();
    let prev_state = std::env::var_os("XDG_STATE_HOME");
    std::env::set_var("XDG_STATE_HOME", tmp.path());

    let toplevel = tmp.path().join("my-repo");
    std::fs::create_dir_all(&toplevel).unwrap();
    let o = UnpinnedOverride {
        claimed: "the-real-project".to_string(),
        used: "my-repo".to_string(),
        toplevel: toplevel.clone(),
    };
    let ns = accept_override(&o).unwrap();
    assert_eq!(ns, Namespace::Project("the-real-project".to_string()));

    // The pin must now appear in the on-disk store.
    let pins = NamespacePins::load();
    assert_eq!(
        pins.pinned(&toplevel),
        Some("the-real-project"),
        "accept_override must persist the pin to the state file"
    );

    match prev_state {
        Some(v) => std::env::set_var("XDG_STATE_HOME", v),
        None => std::env::remove_var("XDG_STATE_HOME"),
    }
}

#[test]
fn accept_override_replacing_a_previous_pin_wins() {
    let _lock = ENV_LOCK.lock().unwrap();
    let tmp = TempDir::new().unwrap();
    let prev_state = std::env::var_os("XDG_STATE_HOME");
    std::env::set_var("XDG_STATE_HOME", tmp.path());

    let toplevel = tmp.path().join("repo");
    std::fs::create_dir_all(&toplevel).unwrap();

    // First acceptance.
    let o1 = UnpinnedOverride {
        claimed: "old-name".to_string(),
        used: "repo".to_string(),
        toplevel: toplevel.clone(),
    };
    accept_override(&o1).unwrap();

    // Second acceptance with a different claimed name.
    let o2 = UnpinnedOverride {
        claimed: "new-name".to_string(),
        used: "repo".to_string(),
        toplevel: toplevel.clone(),
    };
    let ns2 = accept_override(&o2).unwrap();
    assert_eq!(ns2, Namespace::Project("new-name".to_string()));

    let pins = NamespacePins::load();
    assert_eq!(
        pins.pinned(&toplevel),
        Some("new-name"),
        "second accept_override must replace the previous pin"
    );

    match prev_state {
        Some(v) => std::env::set_var("XDG_STATE_HOME", v),
        None => std::env::remove_var("XDG_STATE_HOME"),
    }
}

// ---------------------------------------------------------------------------
// resolve_namespace_in: behaviour not covered by inline tests.
// ---------------------------------------------------------------------------

fn no_git(_: &Path) -> Option<std::path::PathBuf> {
    None
}

fn no_head_config(_: &Path) -> Option<String> {
    None
}

/// Whitespace-only explicit should be treated the same as no explicit —
/// detection falls through to the cwd name.
#[test]
fn whitespace_only_explicit_falls_through_to_detection() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("my-project");
    std::fs::create_dir_all(&dir).unwrap();
    let res = resolve_namespace_in(
        &dir,
        Some("   ".to_string()), // whitespace-only explicit
        &NamespacePins::default(),
        no_git,
        no_head_config,
    );
    // Outside a repo, resolution falls to cwd name.
    assert_eq!(res.namespace, Namespace::Project("my-project".to_string()));
}

/// The nearest CLAUDE.md in a subdirectory is authoritative; an outer
/// CLAUDE.md at the toplevel is not consulted (nearest-wins behavior).
#[test]
fn nearest_claude_md_in_subdirectory_wins_over_toplevel() {
    let tmp = TempDir::new().unwrap();
    let top = tmp.path().join("repo");
    let sub = top.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    // Outer (toplevel) CLAUDE.md claims a namespace.
    std::fs::write(top.join("CLAUDE.md"), "---\nproject: outer-ns\n---\n").unwrap();
    // Inner (subdirectory) CLAUDE.md claims the same name as the toplevel dir.
    std::fs::write(sub.join("CLAUDE.md"), "---\nproject: repo\n---\n").unwrap();

    let git = |_: &Path| -> Option<std::path::PathBuf> { Some(top.clone()) };
    let res = resolve_namespace_in(&sub, None, &NamespacePins::default(), git, no_head_config);
    // sub's CLAUDE.md claims "repo" which matches the toplevel dir name — no pin needed.
    assert_eq!(res.namespace, Namespace::Project("repo".to_string()));
    assert_eq!(res.unpinned_override, None);
}

/// A CLAUDE.md in the middle of the ancestor chain (between start and toplevel)
/// is found before the one at the toplevel.
#[test]
fn claude_md_in_intermediate_ancestor_is_used_not_the_toplevel_one() {
    let tmp = TempDir::new().unwrap();
    let top = tmp.path().join("the-repo");
    let mid = top.join("pkg");
    let start = mid.join("src");
    std::fs::create_dir_all(&start).unwrap();
    // Toplevel CLAUDE.md claims the correct toplevel name.
    std::fs::write(top.join("CLAUDE.md"), "---\nproject: the-repo\n---\n").unwrap();
    // Intermediate CLAUDE.md claims something else (this is the NEAREST one).
    std::fs::write(mid.join("CLAUDE.md"), "---\nproject: interloper\n---\n").unwrap();

    let git = |_: &Path| -> Option<std::path::PathBuf> { Some(top.clone()) };
    let res = resolve_namespace_in(&start, None, &NamespacePins::default(), git, no_head_config);
    // Nearest CLAUDE.md (mid) claims "interloper" != "the-repo" and is unpinned.
    assert_eq!(
        res.namespace,
        Namespace::Project("the-repo".to_string()),
        "unpinned nearest override must degrade to toplevel name"
    );
    let o = res.unpinned_override.expect("override must be surfaced");
    assert_eq!(o.claimed, "interloper");
    assert_eq!(o.used, "the-repo");
}

/// Repo-committed TOML with an empty `namespace` key degrades to the
/// git-toplevel directory name (not an empty-string namespace).
#[test]
fn repo_config_with_empty_namespace_falls_through_to_toplevel_name() {
    let tmp = TempDir::new().unwrap();
    let top = tmp.path().join("my-repo");
    std::fs::create_dir_all(&top).unwrap();
    let git = |_: &Path| -> Option<std::path::PathBuf> { Some(top.clone()) };
    // namespace key present but value is empty after trimming.
    let res = resolve_namespace_in(&top, None, &NamespacePins::default(), git, |_| {
        Some("namespace = \"\"\n".to_string())
    });
    assert_eq!(res.namespace, Namespace::Project("my-repo".to_string()));
}

/// Repo-committed TOML with a whitespace-only namespace value also degrades.
#[test]
fn repo_config_with_whitespace_namespace_falls_through_to_toplevel_name() {
    let tmp = TempDir::new().unwrap();
    let top = tmp.path().join("my-repo");
    std::fs::create_dir_all(&top).unwrap();
    let git = |_: &Path| -> Option<std::path::PathBuf> { Some(top.clone()) };
    let res = resolve_namespace_in(&top, None, &NamespacePins::default(), git, |_| {
        Some("namespace = \"   \"\n".to_string())
    });
    assert_eq!(res.namespace, Namespace::Project("my-repo".to_string()));
}

/// Repo-committed TOML with extra unknown keys still parses the `namespace`.
#[test]
fn repo_config_with_extra_toml_fields_still_uses_namespace() {
    let tmp = TempDir::new().unwrap();
    let top = tmp.path().join("my-repo");
    std::fs::create_dir_all(&top).unwrap();
    let git = |_: &Path| -> Option<std::path::PathBuf> { Some(top.clone()) };
    let res = resolve_namespace_in(&top, None, &NamespacePins::default(), git, |_| {
        Some("namespace = \"the-project\"\n# comment\nextra_key = 42\n".to_string())
    });
    assert_eq!(res.namespace, Namespace::Project("the-project".to_string()));
}

/// Repo-committed TOML with no `namespace` key (other keys only) degrades to
/// the git-toplevel directory name.
#[test]
fn repo_config_with_no_namespace_key_falls_through_to_toplevel_name() {
    let tmp = TempDir::new().unwrap();
    let top = tmp.path().join("my-repo");
    std::fs::create_dir_all(&top).unwrap();
    let git = |_: &Path| -> Option<std::path::PathBuf> { Some(top.clone()) };
    let res = resolve_namespace_in(&top, None, &NamespacePins::default(), git, |_| {
        Some("author = \"alice\"\nversion = \"1.0\"\n".to_string())
    });
    assert_eq!(res.namespace, Namespace::Project("my-repo".to_string()));
}

/// When both a diverging worktree TOML and an unpinned CLAUDE.md frontmatter
/// override are present, both are reported in the same `NamespaceResolution`.
#[test]
fn both_divergence_and_unpinned_override_are_reported_together() {
    let tmp = TempDir::new().unwrap();
    let top = tmp.path().join("my-repo");
    std::fs::create_dir_all(&top).unwrap();
    // Worktree TOML exists but no HEAD blob (untracked).
    std::fs::write(top.join(REPO_CONFIG_FILE), "namespace = \"hijacked\"\n").unwrap();
    // CLAUDE.md claims a different namespace (unpinned override).
    std::fs::write(top.join("CLAUDE.md"), "---\nproject: victim\n---\n").unwrap();
    let git = |_: &Path| -> Option<std::path::PathBuf> { Some(top.clone()) };
    let res = resolve_namespace_in(&top, None, &NamespacePins::default(), git, no_head_config);
    // Safe fallback: toplevel name.
    assert_eq!(res.namespace, Namespace::Project("my-repo".to_string()));
    // Both signals surfaced.
    assert!(
        res.unpinned_override.is_some(),
        "unpinned frontmatter override must be reported"
    );
    assert!(
        res.repo_config_divergence.is_some(),
        "untracked worktree TOML divergence must be reported"
    );
    assert_eq!(
        res.repo_config_divergence.map(|d| d.kind),
        Some(RepoConfigDivergenceKind::Untracked)
    );
}

/// A `NamespaceResolution` for the global namespace has no override and no
/// divergence (nothing was found or rejected).
#[test]
fn global_resolution_has_no_override_and_no_divergence() {
    let res: NamespaceResolution = resolve_namespace_in(
        Path::new("/"),
        None,
        &NamespacePins::default(),
        no_git,
        no_head_config,
    );
    assert_eq!(res.namespace, Namespace::Global);
    assert!(res.unpinned_override.is_none());
    assert!(res.repo_config_divergence.is_none());
}

// ---------------------------------------------------------------------------
// env-override path resolution: empty-string env vars (boundary case).
// ---------------------------------------------------------------------------

#[test]
fn empty_string_env_override_falls_back_to_default() {
    let _lock = ENV_LOCK.lock().unwrap();
    // Point XDG_CONFIG_HOME at an empty temp dir so a developer's real
    // `~/.config/rusty-brain/config.toml` cannot leak path knobs into the
    // resolution under test (env > file > default; we are testing env+file
    // both absent-or-empty).
    let config_isolation = TempDir::new().unwrap();
    let prev_cfg = std::env::var_os("XDG_CONFIG_HOME");
    let prev_sock = std::env::var_os(SOCKET_ENV);
    let prev_db = std::env::var_os(DB_ENV);
    std::env::set_var("XDG_CONFIG_HOME", config_isolation.path());
    std::env::set_var(SOCKET_ENV, "");
    std::env::set_var(DB_ENV, "");
    let sock = resolve_socket_path().unwrap();
    let db = resolve_db_path().unwrap();
    // Defaults computed under the SAME env so the comparison is apples-to-apples.
    let default_sock = default_socket_path().unwrap();
    let default_db = default_db_path().unwrap();
    match prev_cfg {
        Some(v) => std::env::set_var("XDG_CONFIG_HOME", v),
        None => std::env::remove_var("XDG_CONFIG_HOME"),
    }
    match prev_sock {
        Some(v) => std::env::set_var(SOCKET_ENV, v),
        None => std::env::remove_var(SOCKET_ENV),
    }
    match prev_db {
        Some(v) => std::env::set_var(DB_ENV, v),
        None => std::env::remove_var(DB_ENV),
    }
    // Both must equal their defaults (empty string is not a path override).
    assert_eq!(sock, default_sock);
    assert_eq!(db, default_db);
}

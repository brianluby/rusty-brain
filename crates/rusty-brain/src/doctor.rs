//! `rusty-brain doctor`: health checks + diagnostics (doctor/stats PRD DOC-3).
//!
//! Pure check functions produce a [`DoctorReport`]; `run_doctor` orchestrates
//! them against the live system. Doctor NEVER auto-starts the daemon (it
//! diagnoses, it does not mutate) and reads the DB only through the read-only
//! meta helper — zero writer ops, zero side effects.

use std::os::unix::fs::PermissionsExt as _;
use std::path::Path;

/// WAL size above which doctor warns about checkpoint health. SQLite's
/// default autocheckpoint is ~4 MiB of WAL; two orders of magnitude past it
/// means checkpointing is not keeping up (e.g. a long-lived reader pinning
/// the WAL).
pub const WAL_WARN_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckStatus {
    Ok,
    Warn,
    Fail,
    Skipped,
}

impl CheckStatus {
    fn as_str(self) -> &'static str {
        match self {
            CheckStatus::Ok => "ok",
            CheckStatus::Warn => "warn",
            CheckStatus::Fail => "fail",
            CheckStatus::Skipped => "skipped",
        }
    }

    /// Human line marker; FAIL is loud on purpose.
    fn marker(self) -> &'static str {
        match self {
            CheckStatus::Ok => "[ok]  ",
            CheckStatus::Warn => "[warn]",
            CheckStatus::Fail => "[FAIL]",
            CheckStatus::Skipped => "[skip]",
        }
    }
}

/// One health check outcome: what was inspected, what was observed, and (for
/// non-ok outcomes) how to fix it.
#[derive(Debug, Clone)]
pub struct DoctorCheck {
    pub name: &'static str,
    pub status: CheckStatus,
    pub detail: String,
    pub hint: Option<String>,
}

/// The full doctor run. `failed() > 0` drives the non-zero exit.
#[derive(Debug, Clone)]
pub struct DoctorReport {
    pub checks: Vec<DoctorCheck>,
}

impl DoctorReport {
    /// Number of hard failures (warnings and skips do not fail the run).
    pub fn failed(&self) -> usize {
        self.checks
            .iter()
            .filter(|c| c.status == CheckStatus::Fail)
            .count()
    }

    /// Render the report (human lines or a JSON object).
    pub fn render(&self, json: bool) -> String {
        if json {
            let checks: Vec<_> = self
                .checks
                .iter()
                .map(|c| {
                    serde_json::json!({
                        "name": c.name,
                        "status": c.status.as_str(),
                        "detail": c.detail,
                        "hint": c.hint,
                    })
                })
                .collect();
            return serde_json::json!({
                "ok": self.failed() == 0,
                "failed": self.failed(),
                "checks": checks,
            })
            .to_string();
        }
        let mut out = String::new();
        for c in &self.checks {
            out.push_str(&format!("{} {}: {}\n", c.status.marker(), c.name, c.detail));
            if let Some(hint) = &c.hint {
                out.push_str(&format!("       hint: {hint}\n"));
            }
        }
        let failed = self.failed();
        if failed == 0 {
            out.push_str("doctor: no problems found");
        } else {
            out.push_str(&format!(
                "doctor: {failed} problem{} found",
                if failed == 1 { "" } else { "s" }
            ));
        }
        out
    }
}

/// Check that `path` exists with exactly the expected permission bits
/// (`expected` is the 0o777-masked mode, e.g. `0o600`).
pub fn check_mode(name: &'static str, path: &Path, expected: u32) -> DoctorCheck {
    match std::fs::metadata(path) {
        Ok(meta) => {
            let mode = meta.permissions().mode() & 0o777;
            if mode == expected {
                DoctorCheck {
                    name,
                    status: CheckStatus::Ok,
                    detail: format!("{} is {mode:04o}", path.display()),
                    hint: None,
                }
            } else {
                DoctorCheck {
                    name,
                    status: CheckStatus::Fail,
                    detail: format!("{} is {mode:04o} (expected {expected:04o})", path.display()),
                    hint: Some(format!("chmod {expected:o} {}", path.display())),
                }
            }
        }
        Err(e) => DoctorCheck {
            name,
            status: CheckStatus::Fail,
            detail: format!("cannot stat {}: {e}", path.display()),
            hint: None,
        },
    }
}

/// Check the SQLite `-wal` sidecar size: absent or modest is healthy; an
/// oversized WAL means checkpointing is not keeping up.
pub fn check_wal(db_path: &Path) -> DoctorCheck {
    let wal = wal_path(db_path);
    match std::fs::metadata(&wal) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => DoctorCheck {
            name: "wal",
            status: CheckStatus::Ok,
            detail: "no -wal sidecar (fully checkpointed)".to_string(),
            hint: None,
        },
        // Any other stat error means WAL health is unknown, not healthy: the
        // sidecar may exist but be unreadable (e.g. a permission problem).
        Err(e) => DoctorCheck {
            name: "wal",
            status: CheckStatus::Warn,
            detail: format!("cannot stat {}: {e}", wal.display()),
            hint: Some(
                "WAL health is unknown; fix permissions on the DB directory \
                 so doctor can inspect the sidecar"
                    .to_string(),
            ),
        },
        Ok(meta) if meta.len() <= WAL_WARN_BYTES => DoctorCheck {
            name: "wal",
            status: CheckStatus::Ok,
            detail: format!("{} bytes", meta.len()),
            hint: None,
        },
        Ok(meta) => DoctorCheck {
            name: "wal",
            status: CheckStatus::Warn,
            detail: format!(
                "{} is {} bytes (over the {WAL_WARN_BYTES}-byte watermark)",
                wal.display(),
                meta.len()
            ),
            hint: Some(
                "checkpointing is falling behind; close long-lived readers and \
                 restart the daemon (shutdown runs a truncating checkpoint)"
                    .to_string(),
            ),
        },
    }
}

/// The SQLite WAL sidecar path for `db_path` (`<db>-wal`).
fn wal_path(db_path: &Path) -> std::path::PathBuf {
    let mut name = db_path.as_os_str().to_os_string();
    name.push("-wal");
    std::path::PathBuf::from(name)
}

/// Compare the (running or expected) embedding provider identity against the
/// DB's recorded `meta.embedding_model` — the W0.2 fail-closed contract this
/// check diagnoses when the daemon refuses to start.
pub fn check_model_identity(provider: Option<&str>, meta: Option<&str>) -> DoctorCheck {
    match (provider, meta) {
        (Some(p), Some(m)) if p == m => DoctorCheck {
            name: "embedding-model",
            status: CheckStatus::Ok,
            detail: format!("provider '{p}' matches DB meta"),
            hint: None,
        },
        (Some(p), Some(m)) => DoctorCheck {
            name: "embedding-model",
            status: CheckStatus::Fail,
            detail: format!("provider '{p}' does not match DB meta '{m}'"),
            hint: Some(
                "the daemon fails closed on a model swap; either restore the \
                 original provider or opt in with `rusty-brain serve \
                 --accept-model-change` and run `rusty-brain reembed` until \
                 changed=0"
                    .to_string(),
            ),
        },
        (Some(_), None) => DoctorCheck {
            name: "embedding-model",
            status: CheckStatus::Warn,
            detail: "DB has no recorded embedding model (legacy DB; it is \
                     seeded at the next daemon start)"
                .to_string(),
            hint: None,
        },
        (None, meta) => DoctorCheck {
            name: "embedding-model",
            status: CheckStatus::Skipped,
            detail: format!(
                "provider identity unknown offline (DB meta: {})",
                meta.unwrap_or("none")
            ),
            hint: None,
        },
    }
}

/// Warn loudly when the offline deterministic fallback is serving embeddings
/// (recall quality is reduced and vectors are not portable to a real model).
pub fn check_provider(provider_model: &str) -> DoctorCheck {
    if provider_model == "deterministic" {
        DoctorCheck {
            name: "embedding-provider",
            status: CheckStatus::Warn,
            detail: "deterministic fallback active (no VOYAGE_API_KEY and no \
                     local backend): recall quality is reduced and embeddings \
                     are not portable to a real model"
                .to_string(),
            hint: Some(
                "set VOYAGE_API_KEY or configure the local backend, then \
                 restart the daemon"
                    .to_string(),
            ),
        }
    } else {
        DoctorCheck {
            name: "embedding-provider",
            status: CheckStatus::Ok,
            detail: format!("'{provider_model}'"),
            hint: None,
        }
    }
}

/// What model would `serve` pick with the daemon DOWN, given the resolved
/// config? Only the deterministic fallback has a knowable identity offline
/// (`select_provider_kind` parity): a Voyage key or a local backend selects a
/// provider whose model id is provider state we must not guess.
pub fn expected_offline_provider_model(
    api_key: Option<String>,
    embed_backend: Option<&str>,
    local_model: Option<&str>,
) -> Option<String> {
    match crate::serve::select_provider_kind(
        api_key,
        embed_backend.is_some_and(|b| b.eq_ignore_ascii_case("local")) || local_model.is_some(),
    ) {
        crate::serve::ProviderKind::Deterministic => Some("deterministic".to_string()),
        crate::serve::ProviderKind::Voyage | crate::serve::ProviderKind::Local => None,
    }
}

/// The daemon side of a doctor run: the stats payload when the daemon
/// answered, or `None` (with the failure already recorded as a check).
type DaemonStats = Option<(rb_types::MemoryStats, String, bool)>;

/// Daemon reachability — a plain connect (NO auto-start: doctor diagnoses the
/// system as it is). A successful connect also proves the contract version
/// matches (the handshake fails closed on drift).
async fn check_daemon(
    socket_path: &Path,
    namespace: rb_types::Namespace,
    checks: &mut Vec<DoctorCheck>,
) -> DaemonStats {
    let mut client = match rb_proto::Client::connect(socket_path, namespace).await {
        Ok(client) => client,
        Err(e) => {
            checks.push(DoctorCheck {
                name: "daemon",
                status: CheckStatus::Fail,
                detail: format!("not reachable at {}: {e}", socket_path.display()),
                hint: Some(
                    "start it with `rusty-brain serve` (any client command \
                     auto-starts it)"
                        .to_string(),
                ),
            });
            return None;
        }
    };
    match client.stats(None).await {
        Ok(payload) => {
            checks.push(DoctorCheck {
                name: "daemon",
                status: CheckStatus::Ok,
                detail: format!(
                    "reachable at {} (contract v{})",
                    socket_path.display(),
                    rb_proto::CONTRACT_VERSION
                ),
                hint: None,
            });
            Some(payload)
        }
        Err(e) => {
            checks.push(DoctorCheck {
                name: "daemon",
                status: CheckStatus::Fail,
                detail: format!("reachable, but the stats request failed: {e}"),
                hint: Some(
                    "the daemon may predate the stats op; restart it so the \
                     current binary serves the socket"
                        .to_string(),
                ),
            });
            None
        }
    }
}

/// Socket + socket-dir permissions (only meaningful once the socket exists;
/// the daemon binds it 0600 in a 0700 dir, fail-closed).
fn check_socket_permissions(socket_path: &Path, checks: &mut Vec<DoctorCheck>) {
    if socket_path.exists() {
        checks.push(check_mode("socket-mode", socket_path, 0o600));
        if let Some(dir) = socket_path.parent() {
            checks.push(check_mode("socket-dir-mode", dir, 0o700));
        }
    } else {
        checks.push(DoctorCheck {
            name: "socket-mode",
            status: CheckStatus::Skipped,
            detail: format!("{} does not exist (daemon down)", socket_path.display()),
            hint: None,
        });
    }
}

/// DB file permissions + WAL health (rb-store tightens the file to 0600 at
/// every open). The parent dir is deliberately NOT checked: the daemon only
/// owns dirs it created itself, and caller-owned dirs are legitimate (W0.5).
fn check_db_health(db_path: &Path, checks: &mut Vec<DoctorCheck>) {
    if db_path.exists() {
        checks.push(check_mode("db-file-mode", db_path, 0o600));
        checks.push(check_wal(db_path));
    } else {
        checks.push(DoctorCheck {
            name: "db-file-mode",
            status: CheckStatus::Skipped,
            detail: format!(
                "{} does not exist yet (nothing stored so far)",
                db_path.display()
            ),
            hint: None,
        });
    }
}

/// Writer-thread health, as the daemon reported it on the stats reply.
fn check_writer(alive: bool) -> DoctorCheck {
    if alive {
        DoctorCheck {
            name: "writer",
            status: CheckStatus::Ok,
            detail: "writer thread alive".to_string(),
            hint: None,
        }
    } else {
        DoctorCheck {
            name: "writer",
            status: CheckStatus::Fail,
            detail: "writer thread has died; every write will fail".to_string(),
            hint: Some("restart the daemon".to_string()),
        }
    }
}

/// Embedding provider + model-vs-meta identity. With the daemon up, both
/// sides come from its stats reply; with it down, the meta side is read
/// read-only from the DB file and the provider side is only knowable for the
/// deterministic fallback.
fn check_embedding_identity(
    daemon_stats: &DaemonStats,
    db_path: &Path,
    embed_backend: Option<&str>,
    local_model: Option<&str>,
    checks: &mut Vec<DoctorCheck>,
) {
    match daemon_stats {
        Some((stats, provider_model, _)) => {
            checks.push(check_provider(provider_model));
            checks.push(check_model_identity(
                Some(provider_model),
                stats.db_embedding_model.as_deref(),
            ));
        }
        None if db_path.exists() => {
            let meta = match rb_store::read_meta_embedding_model(db_path) {
                Ok(meta) => meta,
                Err(e) => {
                    checks.push(DoctorCheck {
                        name: "embedding-model",
                        status: CheckStatus::Fail,
                        detail: format!("cannot read DB meta: {e}"),
                        hint: None,
                    });
                    return;
                }
            };
            // A legacy DB without a stamp still gets an identity check —
            // `check_model_identity` routes `None` meta to its Warn/Skipped
            // arms, mirroring the daemon-up path.
            let expected = expected_offline_provider_model(
                std::env::var("VOYAGE_API_KEY").ok(),
                embed_backend,
                local_model,
            );
            if let Some(p) = expected.as_deref() {
                checks.push(check_provider(p));
            }
            checks.push(check_model_identity(expected.as_deref(), meta.as_deref()));
        }
        None => {}
    }
}

/// Run the full doctor flow and print the report; `Err` (non-zero exit) when
/// any check FAILS. Never auto-starts the daemon and never writes.
pub async fn run_doctor(
    socket_path: &Path,
    db_path: &Path,
    namespace: rb_types::Namespace,
    embed_backend: Option<String>,
    local_model: Option<String>,
    json: bool,
) -> anyhow::Result<()> {
    let mut checks = Vec::new();

    let daemon_stats = check_daemon(socket_path, namespace, &mut checks).await;
    check_socket_permissions(socket_path, &mut checks);
    check_db_health(db_path, &mut checks);
    if let Some((_, _, alive)) = &daemon_stats {
        checks.push(check_writer(*alive));
    }
    check_embedding_identity(
        &daemon_stats,
        db_path,
        embed_backend.as_deref(),
        local_model.as_deref(),
        &mut checks,
    );

    let report = DoctorReport { checks };
    println!("{}", report.render(json));
    let failed = report.failed();
    if failed > 0 {
        anyhow::bail!(
            "doctor found {failed} problem{}",
            if failed == 1 { "" } else { "s" }
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn check_mode_accepts_expected_and_flags_wrong_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("rb.db");
        std::fs::write(&file, b"x").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600)).unwrap();

        let ok = check_mode("db-file-mode", &file, 0o600);
        assert_eq!(ok.status, CheckStatus::Ok, "{ok:?}");

        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        let fail = check_mode("db-file-mode", &file, 0o600);
        assert_eq!(fail.status, CheckStatus::Fail, "{fail:?}");
        assert!(fail.detail.contains("0644"), "observed mode: {fail:?}");
        let hint = fail.hint.expect("a failing mode check carries guidance");
        assert!(hint.contains("chmod 600"), "remediation: {hint}");
    }

    #[test]
    fn check_mode_fails_on_a_missing_path() {
        let dir = tempfile::tempdir().unwrap();
        let check = check_mode("socket-mode", &dir.path().join("absent"), 0o600);
        assert_eq!(check.status, CheckStatus::Fail, "{check:?}");
    }

    #[test]
    fn check_wal_warns_only_above_the_threshold() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        std::fs::write(&db, b"x").unwrap();

        // No -wal sidecar at all: healthy (fully checkpointed).
        assert_eq!(check_wal(&db).status, CheckStatus::Ok);

        // A small WAL is normal operation.
        let wal = dir.path().join("rb.db-wal");
        std::fs::write(&wal, b"small").unwrap();
        assert_eq!(check_wal(&db).status, CheckStatus::Ok);

        // An oversized WAL warns with a remediation hint (sparse file: set_len
        // reports the logical size without writing the bytes).
        let f = std::fs::OpenOptions::new().write(true).open(&wal).unwrap();
        f.set_len(WAL_WARN_BYTES + 1).unwrap();
        let warn = check_wal(&db);
        assert_eq!(warn.status, CheckStatus::Warn, "{warn:?}");
        let hint = warn.hint.expect("an oversized WAL carries guidance");
        assert!(
            hint.contains("checkpoint") || hint.contains("restart"),
            "remediation: {hint}"
        );
    }

    #[test]
    fn check_wal_flags_an_uninspectable_sidecar_instead_of_claiming_checkpointed() {
        let dir = tempfile::tempdir().unwrap();
        let locked = dir.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        let db = locked.join("rb.db");
        std::fs::write(&db, b"x").unwrap();
        std::fs::write(locked.join("rb.db-wal"), b"wal").unwrap();

        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        // Root ignores directory modes; the EACCES this test needs never fires.
        if std::fs::metadata(locked.join("rb.db-wal")).is_ok() {
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
            return;
        }
        let check = check_wal(&db);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();

        assert_eq!(check.status, CheckStatus::Warn, "{check:?}");
        assert!(check.detail.contains("cannot stat"), "{check:?}");
        assert!(
            check.hint.is_some(),
            "an uninspectable WAL carries guidance"
        );
    }

    #[test]
    fn offline_identity_check_still_reports_a_legacy_db_without_a_model_stamp() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("rb.db");
        // `open` (unlike `open_with_model`) leaves the embedding_model meta
        // unstamped — the legacy-DB shape.
        drop(rb_store::SqliteStore::open(&db, 8).unwrap());

        let mut checks = Vec::new();
        check_embedding_identity(&None, &db, None, None, &mut checks);

        let identity = checks
            .iter()
            .find(|c| c.name == "embedding-model")
            .expect("a legacy DB still gets an embedding-model check offline");
        // Warn when the offline provider is knowable (deterministic), Skipped
        // when it is not (VOYAGE_API_KEY present in the ambient env) — never
        // silently absent.
        assert!(
            matches!(identity.status, CheckStatus::Warn | CheckStatus::Skipped),
            "{identity:?}"
        );
    }

    #[test]
    fn check_model_identity_covers_match_mismatch_and_unknowns() {
        let ok = check_model_identity(Some("deterministic"), Some("deterministic"));
        assert_eq!(ok.status, CheckStatus::Ok, "{ok:?}");

        let fail = check_model_identity(Some("deterministic"), Some("voyage-3"));
        assert_eq!(fail.status, CheckStatus::Fail, "{fail:?}");
        assert!(fail.detail.contains("voyage-3"), "{fail:?}");
        let hint = fail.hint.expect("a model mismatch carries guidance");
        assert!(
            hint.contains("--accept-model-change"),
            "remediation names the opt-in: {hint}"
        );

        // Legacy DB without a stamp: warn, not fail.
        let legacy = check_model_identity(Some("deterministic"), None);
        assert_eq!(legacy.status, CheckStatus::Warn, "{legacy:?}");

        // Provider identity unknown (offline with a remote provider): skip.
        let skipped = check_model_identity(None, Some("voyage-3"));
        assert_eq!(skipped.status, CheckStatus::Skipped, "{skipped:?}");
    }

    #[test]
    fn check_provider_warns_loudly_on_the_deterministic_fallback() {
        let warn = check_provider("deterministic");
        assert_eq!(warn.status, CheckStatus::Warn, "{warn:?}");
        assert!(
            warn.detail.to_lowercase().contains("deterministic"),
            "{warn:?}"
        );

        let ok = check_provider("voyage-3");
        assert_eq!(ok.status, CheckStatus::Ok, "{ok:?}");
    }

    #[test]
    fn expected_offline_provider_model_is_known_only_for_deterministic() {
        // No API key, no local backend: serve would pick deterministic.
        assert_eq!(
            expected_offline_provider_model(None, None, None).as_deref(),
            Some("deterministic")
        );
        assert_eq!(
            expected_offline_provider_model(Some("  ".to_string()), None, None).as_deref(),
            Some("deterministic"),
            "a blank key is not a key (select_provider_kind parity)"
        );
        // A Voyage key or a local backend: the model id is provider state we
        // cannot know offline — the check must skip, not guess.
        assert_eq!(
            expected_offline_provider_model(Some("vk-1".to_string()), None, None),
            None
        );
        assert_eq!(
            expected_offline_provider_model(None, Some("local"), None),
            None
        );
        assert_eq!(
            expected_offline_provider_model(None, None, Some("all-MiniLM-L6-v2")),
            None
        );
    }

    #[test]
    fn report_renders_statuses_hints_and_summary_in_both_modes() {
        let report = DoctorReport {
            checks: vec![
                DoctorCheck {
                    name: "daemon",
                    status: CheckStatus::Ok,
                    detail: "reachable (contract v2)".to_string(),
                    hint: None,
                },
                DoctorCheck {
                    name: "db-file-mode",
                    status: CheckStatus::Fail,
                    detail: "/tmp/rb.db is 0644 (expected 0600)".to_string(),
                    hint: Some("chmod 600 /tmp/rb.db".to_string()),
                },
                DoctorCheck {
                    name: "embedding-provider",
                    status: CheckStatus::Warn,
                    detail: "deterministic fallback active".to_string(),
                    hint: None,
                },
            ],
        };
        let human = report.render(false);
        assert!(human.contains("[ok]"), "{human}");
        assert!(human.contains("[FAIL]"), "{human}");
        assert!(human.contains("[warn]"), "{human}");
        assert!(human.contains("db-file-mode"), "{human}");
        assert!(
            human.contains("chmod 600 /tmp/rb.db"),
            "hint shown: {human}"
        );
        assert!(human.contains("1 problem"), "summary: {human}");

        let json = report.render(true);
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["ok"].as_bool(), Some(false));
        assert_eq!(parsed["failed"].as_u64(), Some(1));
        assert_eq!(parsed["checks"][1]["name"].as_str(), Some("db-file-mode"));
        assert_eq!(parsed["checks"][1]["status"].as_str(), Some("fail"));
        assert_eq!(
            parsed["checks"][1]["hint"].as_str(),
            Some("chmod 600 /tmp/rb.db")
        );
    }

    #[test]
    fn report_counts_failures_for_the_exit_code() {
        let report = DoctorReport {
            checks: vec![
                DoctorCheck {
                    name: "a",
                    status: CheckStatus::Ok,
                    detail: String::new(),
                    hint: None,
                },
                DoctorCheck {
                    name: "b",
                    status: CheckStatus::Warn,
                    detail: String::new(),
                    hint: None,
                },
                DoctorCheck {
                    name: "c",
                    status: CheckStatus::Fail,
                    detail: String::new(),
                    hint: None,
                },
            ],
        };
        assert_eq!(report.failed(), 1, "only Fail drives a non-zero exit");
    }
}

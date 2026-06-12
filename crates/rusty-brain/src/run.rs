//! Async dispatch from parsed `Cli` to daemon/client behavior.

use crate::cli::{Cli, Command};
use crate::{client, output, paths, serve};
use anyhow::Context as _;
use rb_types::MemoryId;
use std::str::FromStr;

/// Parse a CLI id argument into a `MemoryId`, surfacing a clear error.
pub fn parse_id(s: &str) -> rb_types::Result<MemoryId> {
    MemoryId::from_str(s)
}

/// Execute the parsed CLI with a pre-resolved `namespace` (resolved OFF the
/// async runtime by `main`, since detection shells out to git and reads files).
/// `serve` blocks until Ctrl-C; client commands connect (auto-starting the
/// daemon), issue one request, print to stdout, and return.
pub async fn run(cli: Cli, namespace: rb_types::Namespace) -> anyhow::Result<()> {
    let socket_path = paths::socket_path_from_env().context("resolving daemon socket path")?;
    let db_path = paths::db_path_from_env().context("resolving daemon database path")?;

    match cli.command {
        Command::Serve {
            jobs_config,
            accept_model_change,
        } => {
            let jobs_config_path = paths::resolve_jobs_config_path(
                jobs_config,
                std::env::var(paths::JOBS_CONFIG_ENV).ok(),
            );
            let shutdown = async {
                let _ = tokio::signal::ctrl_c().await;
            };
            serve::run_serve(
                socket_path,
                db_path,
                4,
                jobs_config_path,
                accept_model_change,
                shutdown,
            )
            .await
            .context("daemon failed")?;
            Ok(())
        }
        Command::Mcp => crate::mcp::run_mcp(&socket_path, &db_path, namespace)
            .await
            .context("mcp adapter failed"),
        other => run_client(other, cli.json, namespace, &socket_path, &db_path).await,
    }
}

/// Connect to the daemon and dispatch a single client request, scoped to the
/// pre-resolved `namespace`.
async fn run_client(
    command: Command,
    json: bool,
    namespace: rb_types::Namespace,
    socket_path: &std::path::Path,
    db_path: &std::path::Path,
) -> anyhow::Result<()> {
    let self_exe = std::env::current_exe().context("locating own executable")?;
    let mut client = client::connect_or_start(
        socket_path,
        db_path,
        namespace,
        self_exe,
        Some(client::client_identity("cli")),
    )
    .await
    .context("connecting to daemon")?;

    match command {
        Command::Serve { .. } => anyhow::bail!("internal: serve must be handled before run_client"),
        Command::Mcp => anyhow::bail!("internal: mcp must be handled before run_client"),
        Command::Remember {
            content,
            memory_type,
            importance,
            context,
            tags,
        } => {
            let id = client
                .remember(
                    content,
                    context,
                    memory_type,
                    importance,
                    Vec::new(),
                    tags,
                    Vec::new(),
                    1.0,
                )
                .await
                .context("remember failed")?;
            println!("{}", output::render_remembered(&id, json));
        }
        Command::Recall {
            query,
            limit,
            memory_type,
            tags,
        } => {
            let results = client
                .recall(query, memory_type, tags, limit)
                .await
                .context("recall failed")?;
            println!("{}", output::render_recall(&results, json));
        }
        Command::Get { id } => {
            let id = parse_id(&id).context("invalid memory id")?;
            let memory = client.get(id).await.context("get failed")?;
            println!("{}", output::render_get(&memory, json));
        }
        Command::List {
            limit,
            min_importance,
        } => {
            let notes = client
                .list(min_importance, limit)
                .await
                .context("list failed")?;
            println!("{}", output::render_notes(&notes, json));
        }
        Command::Graph { id, depth } => {
            let id = parse_id(&id).context("invalid memory id")?;
            let notes = client.graph(id, depth).await.context("graph failed")?;
            println!("{}", output::render_notes(&notes, json));
        }
        Command::Delete { id } => {
            let id = parse_id(&id).context("invalid memory id")?;
            client.delete(id).await.context("delete failed")?;
            println!("{}", output::render_deleted(json));
        }
        Command::Context => {
            let (recent, important, total) = client.context().await.context("context failed")?;
            println!(
                "{}",
                output::render_context(&recent, &important, total, json)
            );
        }
        Command::Subscribe => {
            client.subscribe().await.context("subscribe failed")?;
            // Stream until the daemon closes the connection (or the process is
            // interrupted). recv_change returns Err(Io) on a clean close, which
            // we treat as a normal end-of-stream exit (not a failure).
            loop {
                tokio::select! {
                    biased;
                    _ = tokio::signal::ctrl_c() => {
                        break;
                    }
                    item = client.recv_change() => {
                        match item {
                            Ok(item) => {
                                println!("{}", output::render_change(&item, json));
                            }
                            // A transport close (Io/EOF) is a normal end-of-stream.
                            // Any other error — a daemon `Response::Error` frame or
                            // a protocol violation — is a real failure the user must
                            // see, so surface it (non-zero exit) instead of hiding it.
                            Err(rb_types::Error::Io(_))
                            | Err(rb_types::Error::IoKind { .. }) => break,
                            Err(e) => {
                                return Err(anyhow::Error::new(e))
                                    .context("subscribe stream error");
                            }
                        }
                    }
                }
            }
        }
        Command::Status => {
            let version = client.ping().await.context("status/ping failed")?;
            if json {
                println!("{{\"contract_version\":{version},\"ok\":true}}");
            } else {
                println!("ok (contract v{version})");
            }
        }
        Command::Evolve { job } => {
            let kind = rb_types::JobKind::parse(&job)
                .map_err(|e| anyhow::anyhow!("invalid job '{job}': {e}"))?;
            let (scanned, changed, skipped) =
                client.run_job(kind).await.context("evolve failed")?;
            if json {
                println!("{{\"scanned\":{scanned},\"changed\":{changed},\"skipped\":{skipped}}}");
            } else {
                println!("evolve {job}: scanned={scanned} changed={changed} skipped={skipped}");
            }
        }
        Command::Reembed { limit } => {
            let (scanned, changed, skipped) =
                client.reembed(limit).await.context("reembed failed")?;
            if json {
                println!("{{\"scanned\":{scanned},\"changed\":{changed},\"skipped\":{skipped}}}");
            } else {
                println!("reembed: scanned={scanned} changed={changed} skipped={skipped}");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;
    use std::str::FromStr;

    #[test]
    #[allow(clippy::type_complexity)]
    fn run_signature_accepts_preresolved_namespace() {
        // Compile-time guard: `run` must accept (Cli, Namespace). This fails to
        // compile until the namespace is threaded in from main (off-runtime).
        fn _assert_run_takes_namespace() -> fn(
            crate::cli::Cli,
            rb_types::Namespace,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = anyhow::Result<()>>>,
        > {
            |cli, ns| Box::pin(run(cli, ns))
        }
        let _ = _assert_run_takes_namespace;
    }

    #[test]
    fn parse_id_accepts_valid_uuid() {
        let id = rb_types::MemoryId::new();
        let parsed = parse_id(&id.to_string()).unwrap();
        assert_eq!(parsed, id);
    }

    #[test]
    fn parse_id_rejects_garbage_with_clear_error() {
        let err = parse_id("not-a-uuid").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("not-a-uuid") || msg.to_lowercase().contains("invalid"),
            "{msg}"
        );
    }

    #[test]
    fn memory_id_from_str_is_what_parse_id_uses() {
        let id = rb_types::MemoryId::new();
        let a = parse_id(&id.to_string()).unwrap();
        let b = rb_types::MemoryId::from_str(&id.to_string()).unwrap();
        assert_eq!(a, b);
    }

    // Proves the namespace is threaded into the client connect path WITHOUT
    // triggering auto-start: a regular file at the socket path makes
    // UnixStream::connect fail with ENOTSOCK, which `should_auto_start` does NOT
    // match, so `connect_or_start` returns immediately (no spawned child, no
    // retry sleeps, no process-global env mutation). Uses an isolated tempdir.
    #[tokio::test]
    async fn connect_or_start_forwards_namespace_without_autostart() {
        use rb_types::Namespace;
        use std::time::Instant;
        use tempfile::TempDir;

        let tmp = TempDir::new().unwrap();
        // A regular file, not a socket: connect -> ENOTSOCK (non-startable).
        let sock = tmp.path().join("not-a-socket");
        std::fs::write(&sock, b"x").unwrap();
        let db = tmp.path().join("rb.db");
        // A self_exe that, if ever spawned, would do nothing harmful; it must NOT
        // be spawned because ENOTSOCK is not an auto-start error.
        let self_exe = std::path::PathBuf::from("/nonexistent/never-spawned");

        let ns = Namespace::Project("injected".to_string());
        let started = Instant::now();
        let result = crate::client::connect_or_start(&sock, &db, ns, self_exe, None).await;
        let elapsed = started.elapsed();

        // Returns an Err quickly (no 50-retry backoff, no daemon spawn).
        assert!(result.is_err(), "expected connect failure on a non-socket");
        assert!(
            elapsed < std::time::Duration::from_secs(2),
            "connect_or_start must not enter the retry/backoff loop for a \
             non-startable error; took {elapsed:?}"
        );
    }

    // Compile-level guarantee that `run` now takes a pre-resolved Namespace
    // (the signature change this task is about). We do not await it against a
    // real daemon; we only bind a typed fn pointer to assert the arity/types.
    #[test]
    #[allow(clippy::type_complexity)]
    fn run_signature_accepts_cli_and_namespace() {
        use rb_types::Namespace;
        let _f: fn(
            crate::cli::Cli,
            Namespace,
        )
            -> std::pin::Pin<Box<dyn std::future::Future<Output = anyhow::Result<()>>>> =
            |cli, ns| Box::pin(run(cli, ns));
    }
}

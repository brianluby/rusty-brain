//! Async dispatch from parsed `Cli` to daemon/client behavior.

use crate::cli::{Cli, Command};
use crate::{client, export, import, output, paths, serve};
use anyhow::Context as _;
use rb_types::MemoryId;
use std::path::Path;
use std::str::FromStr;

/// Parse a CLI id argument into a `MemoryId`, surfacing a clear error.
pub fn parse_id(s: &str) -> rb_types::Result<MemoryId> {
    MemoryId::from_str(s)
}

/// Parse a namespace CLI argument. Full db strings (`global`, `project:NAME`,
/// `session:PROJECT:SID`) parse exactly (fail closed on malformed ones, e.g.
/// `project:`); anything else is a bare project name, matching the
/// `--namespace` flag convention.
pub fn parse_namespace_arg(s: &str) -> rb_types::Result<rb_types::Namespace> {
    let trimmed = s.trim();
    if trimmed.is_empty() {
        return Err(rb_types::Error::InvalidNamespace(s.to_string()));
    }
    if trimmed == "global" || trimmed.starts_with("project:") || trimmed.starts_with("session:") {
        return rb_types::Namespace::parse_db_string(trimmed);
    }
    Ok(rb_types::Namespace::Project(trimmed.to_string()))
}

/// Execute the parsed CLI with a pre-resolved `namespace` (resolved OFF the
/// async runtime by `main`, since detection shells out to git and reads files).
/// `serve` blocks until Ctrl-C; client commands connect (auto-starting the
/// daemon), issue one request, print to stdout, and return.
pub async fn run(cli: Cli, namespace: rb_types::Namespace) -> anyhow::Result<()> {
    // One resolution for every subcommand: env > ~/.config/rusty-brain/
    // config.toml > defaults (C1). A malformed config file fails closed here
    // with the file path in the message; unknown keys only warn.
    let effective = rb_config::EffectiveConfig::resolve().context("resolving configuration")?;
    for warning in &effective.warnings {
        tracing::warn!("{warning}");
    }
    let socket_path = effective.socket_path.clone();
    let db_path = effective.db_path.clone();

    match cli.command {
        Command::Serve {
            jobs_config,
            accept_model_change,
        } => {
            // Flag > env > config file (effective.jobs_config already encodes
            // env > file, so the flag/env composition stays authoritative).
            let jobs_config_path = paths::resolve_jobs_config_path(
                jobs_config,
                std::env::var(paths::JOBS_CONFIG_ENV).ok(),
            )
            .or_else(|| effective.jobs_config.clone());
            let shutdown = async {
                let _ = tokio::signal::ctrl_c().await;
            };
            serve::run_serve(
                effective,
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
    let ns_str = namespace.as_db_string();
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
        Command::Init {
            yes,
            dry_run,
            max_files,
            max_bytes,
            importance,
            undo,
            list_batches,
        } => {
            if list_batches {
                let batches = import::list_batches(db_path);
                println!("{}", output::render_import_batches(&batches, json));
                return Ok(());
            }
            if let Some(batch_id) = undo {
                let counts = import::undo_batch(&mut client, db_path, &batch_id)
                    .await
                    .with_context(|| format!("undoing import batch {batch_id}"))?;
                println!("{}", output::render_import_undo(&batch_id, counts, json));
                return Ok(());
            }

            let root = std::env::current_dir().context("resolving current directory")?;
            let items = import::scan_project(
                &root,
                import::ScanOptions {
                    max_files,
                    max_bytes,
                    importance,
                },
            );
            if items.is_empty() || dry_run {
                println!("{}", output::render_import_plan(&items, json));
                return Ok(());
            }
            if !yes {
                if json {
                    println!("{{\"aborted\":true,\"error\":\"--yes is required with --json\"}}");
                    return Ok(());
                }
                println!("{}", output::render_import_plan(&items, false));
                if !confirm_import(items.len())? {
                    println!("Aborted");
                    return Ok(());
                }
            }
            run_import_items(&mut client, db_path, &items, &[], json).await?;
        }
        Command::Import {
            path,
            memory_type,
            importance,
            tags,
            dry_run,
            max_bytes,
        } => {
            let item = if path == "-" {
                use tokio::io::AsyncReadExt as _;
                let mut buf = Vec::new();
                tokio::io::stdin()
                    .take(u64::try_from(max_bytes).unwrap_or(u64::MAX))
                    .read_to_end(&mut buf)
                    .await
                    .context("reading import stdin")?;
                import::extract_text(
                    "stdin",
                    &String::from_utf8_lossy(&buf),
                    memory_type,
                    importance,
                )
            } else {
                import::extract_file(Path::new(&path), max_bytes, memory_type, importance)
                    .with_context(|| format!("import source {path:?} yielded no text"))?
            };
            let items = vec![item];
            if dry_run {
                println!("{}", output::render_import_plan(&items, json));
                return Ok(());
            }
            run_import_items(&mut client, db_path, &items, &tags, json).await?;
        }
        Command::Export {
            format,
            memory_type,
            tags,
            min_importance,
        } => {
            let filters = export::ExportFilters {
                memory_type,
                tags,
                min_importance,
            };
            let memories = export::fetch_memories(&mut client, &filters).await?;
            let output = export::format_export(&memories, &ns_str, format);
            println!("{output}");
        }
        Command::Backup {
            format,
            retention,
            list,
        } => {
            if list {
                let backups = export::list_backups(db_path);
                println!("{}", output::render_backup_list(&backups, json));
                return Ok(());
            }
            let filters = export::ExportFilters::default();
            let memories = export::fetch_memories(&mut client, &filters).await?;
            let content = export::format_export(&memories, &ns_str, format);
            let path = export::write_backup(db_path, &content, format.extension())
                .context("writing backup")?;
            let pruned = if let Some(n) = retention {
                export::prune_backups(db_path, n).unwrap_or(0)
            } else {
                0
            };
            println!(
                "{}",
                output::render_backup_result(&path, memories.len(), pruned, json)
            );
        }
        Command::Restore {
            path,
            tags,
            dry_run,
        } => {
            let text = if path == "-" {
                use tokio::io::AsyncReadExt as _;
                let mut buf = String::new();
                tokio::io::stdin()
                    .read_to_string(&mut buf)
                    .await
                    .context("reading restore stdin")?;
                buf
            } else {
                std::fs::read_to_string(&path)
                    .with_context(|| format!("reading export file {path:?}"))?
            };
            let items = export::parse_restore(&text).context("parsing export for restore")?;
            if dry_run {
                let plan = output::render_restore_plan(&items, json);
                println!("{plan}");
                return Ok(());
            }
            let counts = export::restore_items(&mut client, &items, &tags)
                .await
                .context("restoring memories")?;
            let batch_id = import::new_batch_id();
            println!(
                "{}",
                output::render_import_result(&batch_id, counts, false, json)
            );
        }
        Command::Remember {
            content,
            memory_type,
            importance,
            context,
            tags,
            supersedes,
            batch,
        } => {
            // CLI `remember` declares no explicit prior: the daemon applies the
            // 1.0 baseline (and an enricher may fill it).
            if batch {
                // Bulk mode: one fact per stdin line, all stored over this single
                // connection (clap guarantees no positional content / --supersedes
                // here). Blank lines are skipped. Reusing one connection avoids a
                // process spawn + handshake per fact when planting large corpora.
                use tokio::io::AsyncBufReadExt as _;
                let mut lines = tokio::io::BufReader::new(tokio::io::stdin()).lines();
                let mut count: u64 = 0;
                while let Some(line) = lines.next_line().await.context("reading --batch stdin")? {
                    let fact = line.trim();
                    if fact.is_empty() {
                        continue;
                    }
                    client
                        .remember(
                            fact.to_string(),
                            context.clone(),
                            memory_type,
                            importance,
                            Vec::new(),
                            tags.clone(),
                            Vec::new(),
                            None,
                        )
                        .await
                        .with_context(|| {
                            format!("remember --batch failed at fact #{}", count + 1)
                        })?;
                    count += 1;
                }
                // Empty/all-blank stdin is a successful no-op (Unix-filter
                // convention: exit 0), but warn on stderr so a mistaken
                // `... | remember --batch` with no input is not silent.
                if count == 0 {
                    eprintln!(
                        "warning: remember --batch read no facts from stdin (empty or all-blank input)"
                    );
                }
                println!("{}", output::render_batch_remembered(count, json));
            } else {
                let content = content
                    .context("remember requires a content argument unless --batch is set")?;
                let id = match supersedes {
                    Some(old) => {
                        let old = parse_id(&old).context("--supersedes must be a memory UUID")?;
                        client
                            .remember_superseding(
                                content,
                                context,
                                memory_type,
                                importance,
                                Vec::new(),
                                tags,
                                Vec::new(),
                                None,
                                old,
                            )
                            .await
                            .context("remember --supersedes failed")?
                    }
                    None => client
                        .remember(
                            content,
                            context,
                            memory_type,
                            importance,
                            Vec::new(),
                            tags,
                            Vec::new(),
                            None,
                        )
                        .await
                        .context("remember failed")?,
                };
                println!("{}", output::render_remembered(&id, json));
            }
        }
        Command::Recall {
            query,
            limit,
            memory_type,
            tags,
        } => {
            let (results, degraded) = client
                .recall_with_status(query, memory_type, tags, limit)
                .await
                .context("recall failed")?;
            if degraded {
                // stderr so `--json` stdout stays machine-parseable. Mirrors
                // the rb-mcp warning: without it a CLI user during an embedder
                // outage sees ordinary-looking, vector-blind results (W1.6d).
                eprintln!(
                    "warning: vector search unavailable (embedding provider \
                     error); results from keyword and graph channels only"
                );
            }
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
        Command::Update {
            id,
            summary,
            importance,
            tags,
            context,
            confidence,
        } => {
            let id = parse_id(&id).context("invalid memory id")?;
            let updates = rb_types::MemoryUpdates {
                content: None,
                summary,
                importance,
                // An absent --tags leaves tags unchanged (the CLI cannot
                // distinguish "clear" from "not provided"; use MCP for that).
                tags: if tags.is_empty() { None } else { Some(tags) },
                context,
                confidence,
            };
            client.update(id, updates).await.context("update failed")?;
            println!("{}", output::render_updated(json));
        }
        Command::Link {
            from,
            to,
            link_type,
            reason,
        } => {
            let from = parse_id(&from).context("invalid `from` memory id")?;
            let to = parse_id(&to).context("invalid `to` memory id")?;
            client
                .link(from, to, link_type, reason)
                .await
                .context("link failed")?;
            println!("{}", output::render_linked(json));
        }
        Command::Feedback { id, kind } => {
            let id = parse_id(&id).context("invalid memory id")?;
            let confidence = client.feedback(id, kind).await.context("feedback failed")?;
            println!("{}", output::render_feedback(confidence, json));
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
        Command::Subscribe { since } => {
            client
                .subscribe_since(since)
                .await
                .context("subscribe failed")?;
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
            let (version, channels) = client.ping_stats().await.context("status/ping failed")?;
            if json {
                match channels {
                    Some(c) => println!(
                        "{{\"contract_version\":{version},\"ok\":true,\"recall_channels\":{{\"recalls\":{},\"fts_hits\":{},\"vector_hits\":{},\"graph_hits\":{}}}}}",
                        c.recalls, c.fts_hits, c.vector_hits, c.graph_hits
                    ),
                    None => println!("{{\"contract_version\":{version},\"ok\":true}}"),
                }
            } else {
                match channels {
                    Some(c) => println!(
                        "ok (contract v{version}) recalls={} fts_hits={} vector_hits={} graph_hits={}",
                        c.recalls, c.fts_hits, c.vector_hits, c.graph_hits
                    ),
                    None => println!("ok (contract v{version})"),
                }
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
        Command::Scrub => {
            let (scanned, redacted, reembed_pending) =
                client.scrub().await.context("scrub failed")?;
            if json {
                println!(
                    "{{\"scanned\":{scanned},\"redacted\":{redacted},\"reembed_pending\":{reembed_pending}}}"
                );
            } else {
                println!(
                    "scrub: scanned={scanned} redacted={redacted} reembed_pending={reembed_pending}"
                );
            }
            if reembed_pending > 0 {
                // stderr so `--json` stdout stays machine-parseable.
                eprintln!(
                    "note: {reembed_pending} memories need re-embedding; run \
                     `rusty-brain reembed` until changed=0 to recompute their vectors"
                );
            }
        }
        Command::Namespace {
            command: crate::cli::NamespaceCommand::Rename { old, new, merge },
        } => {
            let old_ns = parse_namespace_arg(&old).context("invalid OLD namespace")?;
            let new_ns = parse_namespace_arg(&new).context("invalid NEW namespace")?;
            let (moved, vectors) = client
                .rename_namespace(old_ns.clone(), new_ns.clone(), merge)
                .await
                .context("namespace rename failed")?;
            if json {
                println!("{{\"moved\":{moved},\"vectors\":{vectors}}}");
            } else {
                println!(
                    "renamed namespace {} -> {}: moved {moved} memories ({vectors} vectors)",
                    old_ns.as_db_string(),
                    new_ns.as_db_string()
                );
            }
            // stderr so `--json` stdout stays machine-parseable. Sessions
            // resolve their namespace once at handshake (the MCP server at
            // startup), so an agent session that was already open keeps
            // writing under the now-emptied OLD namespace until restarted.
            eprintln!(
                "note: restart any active agent sessions; sessions started \
                 before the rename keep writing to the old namespace (re-run \
                 `namespace rename --merge` to sweep up any stragglers)"
            );
        }
    }
    Ok(())
}

async fn run_import_items(
    client: &mut rb_proto::Client,
    db_path: &Path,
    items: &[import::ImportItem],
    extra_tags: &[String],
    json: bool,
) -> anyhow::Result<()> {
    let batch_id = import::new_batch_id();
    let tag = import::batch_tag(&batch_id);
    let (ids, counts) = import::store_items(client, items, &tag, extra_tags)
        .await
        .context("storing imported memories")?;
    let stored: Vec<MemoryId> = ids.into_iter().flatten().collect();
    if !stored.is_empty() {
        import::write_ledger(db_path, &batch_id, &stored).context("writing import undo ledger")?;
    }
    println!(
        "{}",
        output::render_import_result(&batch_id, counts, false, json)
    );
    Ok(())
}

fn confirm_import(count: usize) -> anyhow::Result<bool> {
    use std::io::Write as _;
    eprint!("Store {count} imported memories? [y/N] ");
    std::io::stderr().flush().context("flushing prompt")?;
    let mut answer = String::new();
    std::io::stdin()
        .read_line(&mut answer)
        .context("reading confirmation")?;
    Ok(matches!(
        answer.trim().to_ascii_lowercase().as_str(),
        "y" | "yes"
    ))
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
    fn parse_namespace_arg_bare_name_is_a_project() {
        // The `--namespace` flag convention: a bare name means project:NAME.
        assert_eq!(
            parse_namespace_arg("my-proj").unwrap(),
            rb_types::Namespace::Project("my-proj".to_string())
        );
        // Whitespace is trimmed like the flag/env paths do.
        assert_eq!(
            parse_namespace_arg("  my-proj  ").unwrap(),
            rb_types::Namespace::Project("my-proj".to_string())
        );
    }

    #[test]
    fn parse_namespace_arg_accepts_full_db_strings() {
        assert_eq!(
            parse_namespace_arg("global").unwrap(),
            rb_types::Namespace::Global
        );
        assert_eq!(
            parse_namespace_arg("project:rusty-brain").unwrap(),
            rb_types::Namespace::Project("rusty-brain".to_string())
        );
        assert_eq!(
            parse_namespace_arg("session:rb:abc").unwrap(),
            rb_types::Namespace::Session {
                project: "rb".to_string(),
                session_id: "abc".to_string(),
            }
        );
    }

    #[test]
    fn parse_namespace_arg_fails_closed_on_malformed_prefixed_forms() {
        // A recognized prefix with a malformed remainder must error, NOT fall
        // back to a literal project named "project:".
        assert!(parse_namespace_arg("project:").is_err());
        assert!(parse_namespace_arg("session:onlyproject").is_err());
        assert!(parse_namespace_arg("").is_err());
        assert!(parse_namespace_arg("   ").is_err());
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

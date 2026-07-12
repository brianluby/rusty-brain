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

/// Assemble the unified [`rb_types::RecallFilter`] from the (identical)
/// `recall`/`list` filter flags — PRD 2026-07-02 search-filter parity. The
/// client splits the legacy-expressible subset back into the pre-filter wire
/// slots, so old daemons keep honoring type/tags/min-importance.
// One positional arg per clap flag, 1:1 and same order as the `Recall`/`List`
// variants — a params struct here would just be a second, drift-prone copy of
// `RecallFilter` itself.
#[allow(clippy::too_many_arguments)]
fn build_recall_filter(
    memory_type: Option<rb_types::MemoryType>,
    tags: Vec<String>,
    min_importance: Option<u8>,
    max_importance: Option<u8>,
    min_confidence: Option<f32>,
    max_confidence: Option<f32>,
    since: Option<chrono::DateTime<chrono::Utc>>,
    until: Option<chrono::DateTime<chrono::Utc>>,
    source: Vec<String>,
    contested: Option<bool>,
    archived: bool,
    anchors: Vec<rb_types::AnchorFilter>,
) -> rb_types::RecallFilter {
    rb_types::RecallFilter {
        types: memory_type.into_iter().collect(),
        tags,
        min_importance,
        max_importance,
        min_confidence,
        max_confidence,
        since,
        until,
        sources: source,
        contested,
        state: if archived {
            rb_types::MemoryState::Archived
        } else {
            rb_types::MemoryState::Active
        },
        anchors,
    }
}

/// Concatenate the per-kind anchor filter flags (`--file`/`--commit`/
/// `--symbol`, each already parse-validated by clap) into the single
/// [`rb_types::RecallFilter::anchors`] list (all-of composition).
fn collect_anchor_filters(
    file: Vec<rb_types::AnchorFilter>,
    commit: Vec<rb_types::AnchorFilter>,
    symbol: Vec<rb_types::AnchorFilter>,
) -> Vec<rb_types::AnchorFilter> {
    file.into_iter().chain(commit).chain(symbol).collect()
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
            http,
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
                http,
                shutdown,
            )
            .await
            .context("daemon failed")?;
            Ok(())
        }
        Command::Mcp => crate::mcp::run_mcp(&socket_path, &db_path, namespace)
            .await
            .context("mcp adapter failed"),
        // Doctor diagnoses the system AS IT IS: it must not auto-start the
        // daemon (run_client would), so it is dispatched before it.
        Command::Doctor => {
            crate::doctor::run_doctor(
                &socket_path,
                &db_path,
                namespace,
                effective.embed_backend.clone(),
                effective.local_model.clone(),
                effective.retention.clone(),
                cli.json,
            )
            .await
        }
        other => {
            run_client(
                other,
                cli.json,
                namespace,
                &socket_path,
                &db_path,
                effective.retention.clone(),
            )
            .await
        }
    }
}

/// Permission bits (0o777-masked) of `path`, or `None` when it cannot be
/// inspected (missing file, permission error).
fn file_mode(path: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::metadata(path)
        .ok()
        .map(|m| m.permissions().mode() & 0o777)
}

/// Connect to the daemon and dispatch a single client request, scoped to the
/// pre-resolved `namespace`.
async fn run_client(
    command: Command,
    json: bool,
    namespace: rb_types::Namespace,
    socket_path: &std::path::Path,
    db_path: &std::path::Path,
    retention: Option<rb_types::RetentionPolicy>,
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
        Command::Doctor => anyhow::bail!("internal: doctor must be handled before run_client"),
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
            let snapshot = export::fetch_memories(&mut client, &filters).await?;
            if snapshot.maybe_truncated {
                eprintln!(
                    "warning: export reached the 100000-memory wire limit and may be incomplete"
                );
            }
            let output = export::format_export(&snapshot.memories, &ns_str, format)?;
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
            let snapshot = export::fetch_memories(&mut client, &filters).await?;
            if snapshot.maybe_truncated {
                eprintln!(
                    "warning: backup reached the 100000-memory wire limit and may be incomplete"
                );
            }
            let content = export::format_export(&snapshot.memories, &ns_str, format)?;
            let path = export::write_backup(db_path, &content, format.extension())
                .context("writing backup")?;
            let pruned = if let Some(n) = retention {
                export::prune_backups(db_path, n).context("pruning old backups")?
            } else {
                0
            };
            println!(
                "{}",
                output::render_backup_result(&path, snapshot.memories.len(), pruned, json)
            );
        }
        Command::Restore {
            path,
            tags,
            dry_run,
        } => {
            let text = if path == "-" {
                use tokio::io::AsyncReadExt as _;
                let mut buf = Vec::new();
                tokio::io::stdin()
                    .take(256 * 1024 * 1024)
                    .read_to_end(&mut buf)
                    .await
                    .context("reading restore stdin")?;
                String::from_utf8_lossy(&buf).into_owned()
            } else {
                let metadata =
                    std::fs::metadata(&path).with_context(|| format!("statting {path:?}"))?;
                if metadata.len() > 256 * 1024 * 1024 {
                    anyhow::bail!(
                        "export file {path:?} is {} MiB; cap is 256 MiB",
                        metadata.len() / (1024 * 1024)
                    );
                }
                std::fs::read_to_string(&path)
                    .with_context(|| format!("reading export file {path:?}"))?
            };
            let items = export::parse_restore(&text).context("parsing export for restore")?;
            if dry_run {
                let plan = output::render_restore_plan(&items, json);
                println!("{plan}");
                return Ok(());
            }
            run_import_items(&mut client, db_path, &items, &tags, json).await?;
        }
        Command::Remember {
            content,
            memory_type,
            importance,
            context,
            tags,
            supersedes,
            file,
            commit,
            symbol,
            batch,
        } => {
            // Typed code anchors: parse-validated per flag; one flat list on
            // the wire. Applied uniformly in --batch mode (like --tags).
            let anchors: Vec<rb_types::MemoryAnchor> =
                file.into_iter().chain(commit).chain(symbol).collect();
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
                        .remember_anchored(
                            fact.to_string(),
                            context.clone(),
                            memory_type,
                            importance,
                            Vec::new(),
                            tags.clone(),
                            Vec::new(),
                            None,
                            anchors.clone(),
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
                let supersedes = supersedes
                    .map(|old| parse_id(&old).context("--supersedes must be a memory UUID"))
                    .transpose()?;
                let id = client
                    .remember_anchored(
                        content,
                        context,
                        memory_type,
                        importance,
                        Vec::new(),
                        tags,
                        Vec::new(),
                        None,
                        anchors,
                        supersedes,
                    )
                    .await
                    .context("remember failed")?;
                println!("{}", output::render_remembered(&id, json));
            }
        }
        Command::Recall {
            query,
            limit,
            memory_type,
            tags,
            min_importance,
            max_importance,
            min_confidence,
            max_confidence,
            since,
            until,
            source,
            contested,
            archived,
            file,
            commit,
            symbol,
        } => {
            let filter = build_recall_filter(
                memory_type,
                tags,
                min_importance,
                max_importance,
                min_confidence,
                max_confidence,
                since,
                until,
                source,
                contested,
                archived,
                collect_anchor_filters(file, commit, symbol),
            );
            let (results, degraded) = client
                .recall_filtered_with_status(query, filter, limit)
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
            memory_type,
            tags,
            min_importance,
            max_importance,
            min_confidence,
            max_confidence,
            since,
            until,
            source,
            contested,
            archived,
            file,
            commit,
            symbol,
        } => {
            let filter = build_recall_filter(
                memory_type,
                tags,
                min_importance,
                max_importance,
                min_confidence,
                max_confidence,
                since,
                until,
                source,
                contested,
                archived,
                collect_anchor_filters(file, commit, symbol),
            );
            let notes = client
                .list_filtered(filter, limit)
                .await
                .context("list failed")?;
            println!("{}", output::render_notes(&notes, json));
        }
        Command::Graph { id, depth } => {
            let id = parse_id(&id).context("invalid memory id")?;
            let notes = client.graph(id, depth).await.context("graph failed")?;
            println!("{}", output::render_notes(&notes, json));
        }
        Command::History { id, depth } => {
            let id = parse_id(&id).context("invalid memory id")?;
            let history = client.history(id, depth).await.context("history failed")?;
            println!("{}", output::render_history(&history, json));
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
            // DOC-1: one health payload — writer health, embedding identity,
            // DB path/mode, WAL size, corpus counts. Falls back to the legacy
            // ping-only line against a daemon that predates the stats op (it
            // closes the connection on the unknown frame).
            match client.stats(None).await {
                Ok((stats, provider_model, writer_alive)) => {
                    let view = output::StatusView {
                        contract_version: version,
                        recall_channels: channels,
                        stats: &stats,
                        provider_model: &provider_model,
                        writer_alive,
                        db_file_mode: file_mode(Path::new(&stats.db_path)),
                    };
                    println!("{}", output::render_status(&view, json));
                }
                Err(e) => {
                    // stderr so `--json` stdout stays machine-parseable.
                    eprintln!(
                        "note: daemon did not answer the stats request ({e}); \
                         it may predate this binary — restart it for full status"
                    );
                    if json {
                        let mut value = serde_json::json!({
                            "contract_version": version,
                            "ok": true,
                        });
                        if let (Some(c), Some(obj)) = (channels, value.as_object_mut()) {
                            obj.insert(
                                "recall_channels".to_string(),
                                serde_json::json!({
                                    "recalls": c.recalls,
                                    "fts_hits": c.fts_hits,
                                    "vector_hits": c.vector_hits,
                                    "graph_hits": c.graph_hits,
                                }),
                            );
                        }
                        println!("{value}");
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
            }
        }
        Command::Stats { window_days } => {
            let (stats, provider_model, writer_alive) =
                client.stats(window_days).await.context("stats failed")?;
            println!(
                "{}",
                output::render_stats(&stats, &provider_model, writer_alive, json)
            );
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
        Command::Forget {
            dry_run,
            apply,
            hard,
            yes,
        } => {
            // No [retention] section = no policy = nothing to do — a clear
            // error, not a silent no-op (fail closed both ways).
            let Some(policy) = retention else {
                anyhow::bail!(
                    "retention is not configured: add a [retention] section to {} \
                     (see the retention PRD; forgetting is opt-in)",
                    rb_config::config_file_path()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|_| "~/.config/rusty-brain/config.toml".to_string())
                );
            };
            let mode = if hard {
                rb_types::ForgetMode::Hard
            } else {
                rb_types::ForgetMode::Apply
            };
            let execute = (apply || hard) && !dry_run;
            if execute {
                // Friendly client-side refusal; the daemon AND the store
                // refuse a disabled policy too (defense in depth).
                if !policy.enabled {
                    anyhow::bail!(
                        "retention.enabled = false: refusing to execute; run \
                         `rusty-brain forget --dry-run` to preview, then set \
                         retention.enabled = true to allow forgetting"
                    );
                }
                // Hard execution is bulk + cascading + irreversible: gate it
                // on an interactive confirmation or an explicit --yes
                // (PR #60 review; the import-confirmation precedent). The
                // preview shown is the SAME plan query the sweep executes.
                if hard {
                    use std::io::IsTerminal as _;
                    match hard_execute_gate(yes, json, std::io::stdin().is_terminal()) {
                        HardGate::Proceed => {}
                        HardGate::Refuse(msg) => anyhow::bail!(msg),
                        HardGate::Prompt => {
                            let plan = client
                                .forget_plan(policy.clone(), mode)
                                .await
                                .context("forget preview failed")?;
                            if plan.candidates.is_empty() {
                                println!("{}", output::render_forget_plan(&plan, false));
                                return Ok(());
                            }
                            eprintln!("{}", output::render_forget_plan(&plan, false));
                            if !confirm_hard_forget(plan.candidates.len(), plan.total_eligible)? {
                                println!("Aborted");
                                return Ok(());
                            }
                        }
                    }
                }
                let outcome = client
                    .forget_execute(policy, mode)
                    .await
                    .context("forget failed")?;
                println!("{}", output::render_forget_outcome(&outcome, json));
                if let Some(failure) = &outcome.failure {
                    // The counts above committed durably; exit non-zero so
                    // automation notices the pass did not complete.
                    anyhow::bail!(
                        "forget pass incomplete after {} archived / {} purged: {failure}",
                        outcome.archived,
                        outcome.purged
                    );
                }
                if outcome.remaining > 0 {
                    // stderr so `--json` stdout stays machine-parseable.
                    eprintln!(
                        "note: {} eligible memories remain (bounded pass); \
                         re-run to continue",
                        outcome.remaining
                    );
                }
            } else {
                let plan = client
                    .forget_plan(policy, mode)
                    .await
                    .context("forget dry-run failed")?;
                println!("{}", output::render_forget_plan(&plan, json));
                if !plan.candidates.is_empty() {
                    let flag = if hard { "--hard" } else { "--apply" };
                    eprintln!(
                        "dry-run only — nothing was changed; run `rusty-brain forget {flag}` \
                         to execute"
                    );
                }
            }
        }
        Command::Review {
            dry_run,
            apply,
            policy,
            interactive,
            since,
            limit,
            threshold,
            snooze_days,
            yes,
        } => {
            use std::io::IsTerminal as _;
            match review_gate(
                dry_run,
                interactive,
                apply,
                yes,
                json,
                std::io::stdin().is_terminal(),
            ) {
                ReviewGate::Refuse(msg) => anyhow::bail!(msg),
                ReviewGate::DryRun => {
                    let plan = client
                        .review_plan(policy, since, limit, threshold)
                        .await
                        .context("review dry-run failed")?;
                    let has_items = !plan.items.is_empty();
                    println!("{}", output::render_review_plan(&plan, json));
                    if has_items && !json {
                        // stderr so `--json` stdout stays machine-parseable.
                        eprintln!(
                            "dry-run only — nothing was changed; run `rusty-brain review \
                             --interactive` to walk the queue or `--apply --policy <name>` \
                             to execute"
                        );
                    }
                }
                gate @ (ReviewGate::ApplyProceed | ReviewGate::ApplyPrompt) => {
                    // clap enforces `--apply requires --policy`; fail closed
                    // anyway rather than sweep policy-less.
                    let Some(policy) = policy else {
                        anyhow::bail!("review --apply requires --policy");
                    };
                    if matches!(gate, ReviewGate::ApplyPrompt) {
                        // The preview shown is the SAME queue + policy plan
                        // the apply pass derives (shared plan, REV-3).
                        let plan = client
                            .review_plan(Some(policy), since, limit, threshold)
                            .await
                            .context("review preview failed")?;
                        if plan.planned.is_empty() {
                            println!("{}", output::render_review_plan(&plan, false));
                            println!("review ({}): nothing to do", policy.as_str());
                            return Ok(());
                        }
                        eprintln!("{}", output::render_review_plan(&plan, false));
                        if !confirm_review_apply(plan.planned.len(), plan.items.len())? {
                            println!("Aborted");
                            return Ok(());
                        }
                    }
                    let outcome = client
                        .review_apply(policy, since, limit, threshold)
                        .await
                        .context("review apply failed")?;
                    println!("{}", output::render_review_outcome(&outcome, json));
                    if let Some(failure) = &outcome.failure {
                        // The counts above committed durably; exit non-zero
                        // so automation notices the pass did not complete.
                        anyhow::bail!("review pass incomplete: {failure}");
                    }
                }
                ReviewGate::Interactive => {
                    review_interactive(&mut client, since, limit, threshold, snooze_days).await?;
                }
            }
        }
        Command::Scrub => {
            let outcome = client
                .scrub_with_checkpoint()
                .await
                .context("scrub failed")?;
            let checkpoint = outcome.wal_checkpoint;
            let plaintext_may_remain = outcome.plaintext_may_remain_in_wal();
            let warning = "the WAL checkpoint was busy or its status was unavailable; \
                           pre-redaction plaintext may remain in the WAL";
            let remediation = "close long-lived rusty-brain database readers, then rerun \
                               `rusty-brain scrub` to retry the truncating WAL checkpoint";
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "scanned": outcome.scanned,
                        "redacted": outcome.redacted,
                        "reembed_pending": outcome.reembed_pending,
                        "wal_checkpoint": checkpoint.map(|status| serde_json::json!({
                            "busy": status.busy,
                            "log_frames": status.log_frames,
                            "checkpointed_frames": status.checkpointed_frames,
                        })),
                        "plaintext_may_remain_in_wal": plaintext_may_remain,
                        "warning": plaintext_may_remain.then_some(warning),
                        "remediation": plaintext_may_remain.then_some(remediation),
                    })
                );
            } else {
                let checkpoint_status = checkpoint.map_or_else(
                    || "unavailable".to_string(),
                    |status| {
                        format!(
                            "{} log_frames={} checkpointed_frames={}",
                            if status.busy { "busy" } else { "complete" },
                            status.log_frames,
                            status.checkpointed_frames
                        )
                    },
                );
                println!(
                    "scrub: scanned={} redacted={} reembed_pending={} wal_checkpoint={checkpoint_status}",
                    outcome.scanned, outcome.redacted, outcome.reembed_pending
                );
            }
            if plaintext_may_remain {
                // stderr keeps JSON stdout machine-parseable while the JSON
                // payload independently exposes the same risk/remediation.
                eprintln!("warning: {warning}; {remediation}");
            }
            if outcome.reembed_pending > 0 {
                // stderr so `--json` stdout stays machine-parseable.
                eprintln!(
                    "note: {} memories need re-embedding; run \
                     `rusty-brain reembed` until changed=0 to recompute their vectors",
                    outcome.reembed_pending
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

/// The interactive review loop (REV-2): fetch the queue once, then walk it
/// item by item — render both sides, read one decision, apply it through the
/// per-item `Resolve` op (each resolution is atomic daemon-side). Decisions
/// are parsed by the pure [`parse_review_choice`]; this function is only the
/// I/O shell around it.
async fn review_interactive(
    client: &mut rb_proto::Client,
    since: Option<u64>,
    limit: Option<u32>,
    threshold: Option<f32>,
    snooze_days: u32,
) -> anyhow::Result<()> {
    use std::io::Write as _;
    let plan = client
        .review_plan(None, since, limit, threshold)
        .await
        .context("review queue fetch failed")?;
    if plan.items.is_empty() {
        println!("{}", output::render_review_plan(&plan, false));
        return Ok(());
    }
    let total = plan.items.len();
    let mut resolved = 0usize;
    'items: for (i, item) in plan.items.iter().enumerate() {
        println!("{}", output::render_review_item(item, i + 1, total));
        let action = loop {
            eprint!("[k]eep [b]ump [m]erge [a]rchive [d]emote [s]nooze [x]skip [q]uit > ");
            std::io::stderr().flush().context("flushing prompt")?;
            let mut line = String::new();
            let read = std::io::stdin()
                .read_line(&mut line)
                .context("reading answer")?;
            if read == 0 {
                // EOF (e.g. Ctrl-D): stop cleanly, nothing half-applied.
                break 'items;
            }
            match parse_review_choice(&line, item.members.len(), item.reason) {
                Ok(ReviewChoice::Quit) => break 'items,
                Ok(ReviewChoice::Skip) => continue 'items,
                Ok(choice) => match choice_to_action(choice, item, snooze_days) {
                    Some(action) => break action,
                    None => continue 'items,
                },
                Err(msg) => {
                    eprintln!("{msg}");
                    continue;
                }
            }
        };
        let ids: Vec<rb_types::MemoryId> = item.members.iter().map(|m| m.id.clone()).collect();
        match client
            .resolve_review_item(item.reason, ids, action, threshold)
            .await
        {
            Ok(resolution) => {
                resolved += 1;
                println!("{}", output::render_review_resolution(&resolution, false));
            }
            Err(e) => {
                // One failed resolution never aborts the session; the item
                // stays in the queue for the next sweep.
                eprintln!("resolution failed (item left in the queue): {e}");
            }
        }
    }
    println!("review: {resolved} of {total} item(s) resolved");
    Ok(())
}

/// Outcome of the review-mode gate (PRD 2026-07-02; the `hard_execute_gate`
/// matrix pattern). Pure so every cell is testable.
#[derive(Debug)]
enum ReviewGate {
    /// Default posture: list the queue (or the policy plan), write nothing.
    DryRun,
    /// Walk the queue item by item on a real terminal.
    Interactive,
    /// `--apply --yes`: execute without prompting (automation).
    ApplyProceed,
    /// `--apply` on an interactive TTY without `--yes`: show the plan and
    /// ask (default NO).
    ApplyPrompt,
    /// Machine context without `--yes`, or interactive without a terminal.
    Refuse(String),
}

/// Decide how a `review` invocation may proceed. Mirrors `hard_execute_gate`:
/// a batch mutation must never ride an unattended pipeline by accident, and
/// the per-item loop needs a human at a terminal. An EXPLICIT `--dry-run`
/// wins over everything (belt-and-suspenders under the clap conflict — a
/// preview invocation must never mutate; PR #63 Copilot finding).
fn review_gate(
    dry_run: bool,
    interactive: bool,
    apply: bool,
    yes: bool,
    json: bool,
    stdin_is_tty: bool,
) -> ReviewGate {
    if dry_run {
        return ReviewGate::DryRun;
    }
    if interactive {
        if json {
            return ReviewGate::Refuse(
                "refusing interactive review: --json is machine output; drop --json or use \
                 --apply --policy"
                    .to_string(),
            );
        }
        if !stdin_is_tty {
            return ReviewGate::Refuse(
                "refusing interactive review: stdin is not a terminal; use --apply --policy \
                 for scripted runs"
                    .to_string(),
            );
        }
        return ReviewGate::Interactive;
    }
    if apply {
        if yes {
            return ReviewGate::ApplyProceed;
        }
        if json {
            return ReviewGate::Refuse(
                "refusing review apply: --json is non-interactive; pass --yes to confirm \
                 the sweep"
                    .to_string(),
            );
        }
        if !stdin_is_tty {
            return ReviewGate::Refuse(
                "refusing review apply: stdin is non-interactive; pass --yes to confirm \
                 the sweep"
                    .to_string(),
            );
        }
        return ReviewGate::ApplyPrompt;
    }
    ReviewGate::DryRun
}

/// Interactive confirmation for a review apply pass (the `confirm_import`
/// precedent): default is NO.
fn confirm_review_apply(actions: usize, total: usize) -> anyhow::Result<bool> {
    use std::io::Write as _;
    eprint!("Apply {actions} planned action(s) across {total} queue item(s)? [y/N] ");
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

/// One parsed interactive decision. Pure data so the grammar is testable
/// without a terminal (the `hard_execute_gate` pattern: decisions pure, I/O
/// at the edge).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReviewChoice {
    Keep,
    KeepBump,
    Merge,
    /// Archive the 0-based member index.
    Archive(usize),
    /// Demote the 0-based member index.
    Demote(usize),
    Snooze,
    Skip,
    Quit,
}

/// Parse one interactive answer against an item with `member_count` members
/// and `reason`. Garbage is a clear `Err` message, never a panic and never a
/// silent mutation; an empty answer skips (the safe default). Merge is
/// near-duplicate-only (mirroring the daemon's `ReviewAction::validate`
/// restriction) so the user gets the redirect locally, without a round trip.
fn parse_review_choice(
    input: &str,
    member_count: usize,
    reason: rb_types::ReviewReason,
) -> Result<ReviewChoice, String> {
    const HELP: &str = "expected k(eep), b(ump), m(erge), a(rchive) [1|2], d(emote) [1|2], \
                        s(nooze), x/enter to skip, or q(uit)";
    let normalized = input.trim().to_ascii_lowercase();
    let mut parts = normalized.split_whitespace();
    let verb = parts.next().unwrap_or("");
    let arg = parts.next();
    if parts.next().is_some() {
        return Err(format!("too many words in {input:?}: {HELP}"));
    }

    let side = |arg: Option<&str>| -> Result<usize, String> {
        match (member_count, arg) {
            (1, None) => Ok(0),
            (_, Some(n)) => match n.parse::<usize>() {
                Ok(i) if (1..=member_count).contains(&i) => Ok(i - 1),
                _ => Err(format!("side must be 1..={member_count}, got {n:?}")),
            },
            (_, None) => Err(format!("pick a side: 1..={member_count}")),
        }
    };

    match (verb, arg) {
        ("" | "x" | "skip", None) => Ok(ReviewChoice::Skip),
        ("q" | "quit", None) => Ok(ReviewChoice::Quit),
        ("k" | "keep", None) => Ok(ReviewChoice::Keep),
        ("b" | "bump", None) => Ok(ReviewChoice::KeepBump),
        ("m" | "merge", None) => {
            if reason != rb_types::ReviewReason::NearDuplicate {
                return Err(
                    "merge applies to near-duplicate items only; resolve this one with \
                     keep, archive, demote, or snooze"
                        .to_string(),
                );
            }
            if member_count == 2 {
                Ok(ReviewChoice::Merge)
            } else {
                Err("merge needs a two-sided item".to_string())
            }
        }
        ("a" | "archive", arg) => side(arg).map(ReviewChoice::Archive),
        ("d" | "demote", arg) => side(arg).map(ReviewChoice::Demote),
        ("s" | "snooze", None) => Ok(ReviewChoice::Snooze),
        _ => Err(format!("unrecognized answer {input:?}: {HELP}")),
    }
}

/// Map a non-skip choice onto the wire action for `item`.
fn choice_to_action(
    choice: ReviewChoice,
    item: &rb_types::ReviewItem,
    snooze_days: u32,
) -> Option<rb_types::ReviewAction> {
    match choice {
        ReviewChoice::Keep => Some(rb_types::ReviewAction::Keep { bump: false }),
        ReviewChoice::KeepBump => Some(rb_types::ReviewAction::Keep { bump: true }),
        ReviewChoice::Merge => Some(rb_types::ReviewAction::Merge),
        ReviewChoice::Archive(i) => item
            .members
            .get(i)
            .map(|m| rb_types::ReviewAction::Archive { id: m.id.clone() }),
        ReviewChoice::Demote(i) => item
            .members
            .get(i)
            .map(|m| rb_types::ReviewAction::Demote { id: m.id.clone() }),
        ReviewChoice::Snooze => Some(rb_types::ReviewAction::Snooze { days: snooze_days }),
        ReviewChoice::Skip | ReviewChoice::Quit => None,
    }
}

/// Outcome of the hard-forget confirmation gate (PR #60 review).
#[derive(Debug)]
enum HardGate {
    /// `--yes` was passed: proceed without prompting.
    Proceed,
    /// Interactive TTY without `--yes`: show the plan and prompt.
    Prompt,
    /// Machine context without `--yes`: refuse with this message.
    Refuse(String),
}

/// Decide how a hard EXECUTE may proceed. Pure so the matrix is testable:
/// `--yes` always proceeds; without it, `--json` and non-TTY stdin refuse
/// loudly (a purge must never ride an unattended pipeline by accident), and
/// only an interactive terminal gets the prompt.
fn hard_execute_gate(yes: bool, json: bool, stdin_is_tty: bool) -> HardGate {
    if yes {
        return HardGate::Proceed;
    }
    if json {
        return HardGate::Refuse(
            "refusing hard forget: --json is non-interactive; pass --yes to confirm the purge"
                .to_string(),
        );
    }
    if !stdin_is_tty {
        return HardGate::Refuse(
            "refusing hard forget: stdin is non-interactive; pass --yes to confirm the purge"
                .to_string(),
        );
    }
    HardGate::Prompt
}

/// Interactive confirmation for a hard forget (the `confirm_import`
/// precedent): default is NO.
fn confirm_hard_forget(count: usize, total: u64) -> anyhow::Result<bool> {
    use std::io::Write as _;
    eprint!(
        "PERMANENTLY purge {count} memories (of {total} eligible)? \
         This cannot be undone. [y/N] "
    );
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
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use std::str::FromStr;

    #[test]
    fn review_gate_matrix_mirrors_the_hard_execute_gate() {
        // Interactive mode needs a real terminal and human-readable output.
        assert!(matches!(
            review_gate(false, true, false, false, false, true),
            ReviewGate::Interactive
        ));
        assert!(matches!(
            review_gate(false, true, false, false, true, true),
            ReviewGate::Refuse(_)
        ));
        assert!(matches!(
            review_gate(false, true, false, false, false, false),
            ReviewGate::Refuse(_)
        ));

        // Apply: --yes proceeds anywhere; without it, --json and piped stdin
        // refuse loudly, and only a TTY gets the default-NO prompt.
        assert!(matches!(
            review_gate(false, false, true, true, true, false),
            ReviewGate::ApplyProceed
        ));
        assert!(matches!(
            review_gate(false, false, true, false, false, true),
            ReviewGate::ApplyPrompt
        ));
        match review_gate(false, false, true, false, true, true) {
            ReviewGate::Refuse(msg) => assert!(msg.contains("--yes"), "{msg}"),
            other => panic!("json apply without --yes must refuse, got {other:?}"),
        }
        match review_gate(false, false, true, false, false, false) {
            ReviewGate::Refuse(msg) => assert!(msg.contains("--yes"), "{msg}"),
            other => panic!("piped apply without --yes must refuse, got {other:?}"),
        }

        // Neither flag: the dry-run listing, everywhere.
        assert!(matches!(
            review_gate(false, false, false, false, true, false),
            ReviewGate::DryRun
        ));
        assert!(matches!(
            review_gate(false, false, false, false, false, true),
            ReviewGate::DryRun
        ));

        // Copilot finding (PR #63), belt-and-suspenders under the clap
        // conflict: an EXPLICIT --dry-run wins over every mutating flag —
        // even a hostile flag combination can only preview.
        assert!(matches!(
            review_gate(true, false, true, true, false, true),
            ReviewGate::DryRun
        ));
        assert!(matches!(
            review_gate(true, true, false, false, false, true),
            ReviewGate::DryRun
        ));
    }

    #[test]
    fn parse_review_choice_covers_the_action_set() {
        use ReviewChoice as C;
        // Pair items: every action, both long and short spellings.
        for (input, expected) in [
            ("k", C::Keep),
            ("keep", C::Keep),
            ("b", C::KeepBump),
            ("bump", C::KeepBump),
            ("m", C::Merge),
            ("merge", C::Merge),
            ("a 1", C::Archive(0)),
            ("archive 2", C::Archive(1)),
            ("d 2", C::Demote(1)),
            ("demote 1", C::Demote(0)),
            ("s", C::Snooze),
            ("snooze", C::Snooze),
            ("", C::Skip),
            ("x", C::Skip),
            ("skip", C::Skip),
            ("q", C::Quit),
            ("quit", C::Quit),
            ("  K  ", C::Keep),
        ] {
            assert_eq!(
                parse_review_choice(input, 2, rb_types::ReviewReason::NearDuplicate),
                Ok(expected),
                "{input:?}"
            );
        }
        // Single-member items: the side index is implicit.
        let low = rb_types::ReviewReason::LowConfidence;
        assert_eq!(parse_review_choice("a", 1, low), Ok(C::Archive(0)));
        assert_eq!(parse_review_choice("d", 1, low), Ok(C::Demote(0)));
    }

    #[test]
    fn parse_review_choice_restricts_merge_to_near_duplicates() {
        // PR #63 review: the interactive loop must mirror the daemon's
        // reason restriction — merging a contradiction pair gets a clear
        // redirect instead of a round trip to a daemon rejection.
        let err = parse_review_choice("m", 2, rb_types::ReviewReason::Contradiction).unwrap_err();
        assert!(err.contains("near-duplicate"), "{err}");
        assert!(
            err.contains("keep") && err.contains("archive"),
            "the error redirects to the valid actions: {err}"
        );
    }

    #[test]
    fn parse_review_choice_rejects_garbage_without_panicking() {
        // Garbage never panics and never silently maps to a mutation.
        for (input, members) in [
            ("m", 1),                      // merge needs a pair
            ("a", 2),                      // pair archive needs a side
            ("d", 2),                      // pair demote needs a side
            ("a 3", 2),                    // out of range
            ("a 0", 2),                    // 1-based
            ("a banana", 2),               // not a number
            ("archive 1 2", 2),            // trailing garbage
            ("yolo", 2),                   // unknown verb
            ("merge now", 2),              // trailing garbage
            ("a 99999999999999999999", 2), // overflow
        ] {
            let got = parse_review_choice(input, members, rb_types::ReviewReason::NearDuplicate);
            assert!(got.is_err(), "{input:?} must be rejected, got {got:?}");
        }
    }

    #[test]
    fn hard_forget_gate_requires_confirmation_or_explicit_yes() {
        // PR #60 review (MEDIUM): bulk + cascading + irreversible warrants
        // the import-confirmation precedent. --yes proceeds anywhere;
        // without it, machine contexts (--json or non-TTY stdin) REFUSE with
        // a clear error instead of hanging or silently proceeding, and only
        // an interactive TTY gets the prompt.
        assert!(matches!(
            hard_execute_gate(true, false, true),
            HardGate::Proceed
        ));
        assert!(matches!(
            hard_execute_gate(true, true, false),
            HardGate::Proceed
        ));
        assert!(matches!(
            hard_execute_gate(false, false, true),
            HardGate::Prompt
        ));
        match hard_execute_gate(false, true, true) {
            HardGate::Refuse(msg) => assert!(msg.contains("--yes"), "{msg}"),
            other => panic!("json without --yes must refuse, got {other:?}"),
        }
        match hard_execute_gate(false, false, false) {
            HardGate::Refuse(msg) => {
                assert!(msg.contains("--yes"), "{msg}");
                assert!(msg.contains("non-interactive"), "{msg}");
            }
            other => panic!("non-TTY without --yes must refuse, got {other:?}"),
        }
    }

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

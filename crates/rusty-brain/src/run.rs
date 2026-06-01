//! Async dispatch from parsed `Cli` to daemon/client behavior.

use crate::cli::{Cli, Command};
use crate::namespace_detect::detect_namespace;
use crate::{client, output, paths, serve};
use anyhow::Context as _;
use rb_types::MemoryId;
use std::str::FromStr;

/// Parse a CLI id argument into a `MemoryId`, surfacing a clear error.
pub fn parse_id(s: &str) -> rb_types::Result<MemoryId> {
    MemoryId::from_str(s)
}

/// Execute the parsed CLI. `serve` blocks until Ctrl-C; client commands connect
/// (auto-starting the daemon), issue one request, print to stdout, and return.
pub async fn run(cli: Cli) -> anyhow::Result<()> {
    let socket_path = paths::socket_path_from_env().context("resolving daemon socket path")?;
    let db_path = paths::db_path_from_env().context("resolving daemon database path")?;

    match cli.command {
        Command::Serve => {
            let shutdown = async {
                let _ = tokio::signal::ctrl_c().await;
            };
            serve::run_serve(socket_path, db_path, 4, shutdown)
                .await
                .context("daemon failed")?;
            Ok(())
        }
        other => run_client(other, cli.json, &socket_path, &db_path).await,
    }
}

/// Connect to the daemon and dispatch a single client request.
async fn run_client(
    command: Command,
    json: bool,
    socket_path: &std::path::Path,
    db_path: &std::path::Path,
) -> anyhow::Result<()> {
    let namespace = detect_namespace();
    let self_exe = std::env::current_exe().context("locating own executable")?;
    let mut client = client::connect_or_start(socket_path, db_path, namespace.clone(), self_exe)
        .await
        .context("connecting to daemon")?;

    match command {
        Command::Serve => anyhow::bail!("internal: serve must be handled before run_client"),
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
                .recall(query, Some(namespace), memory_type, tags, limit)
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
                .list(Some(namespace), min_importance, limit)
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
        Command::Status => {
            let version = client.ping().await.context("status/ping failed")?;
            if json {
                println!("{{\"contract_version\":{version},\"ok\":true}}");
            } else {
                println!("ok (contract v{version})");
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
}

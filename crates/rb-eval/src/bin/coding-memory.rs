//! Categorized #85 baseline; capability failures are data, execution failures exit 2.
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "Offline coding-memory evidence baseline (not answer-quality scoring)")]
struct Args {
    /// Strict real-vector replay fixture; defaults to the isolated #85 recording.
    #[arg(
        long,
        default_value = "crates/rb-eval/fixtures/embeddings/coding-memory-all-MiniLM-L6-v2.json"
    )]
    vectors: PathBuf,
    /// Write the same complete categorized JSON report to this path.
    #[arg(long)]
    output: Option<PathBuf>,
}

/// Emit all observations without converting an unsupported capability into a pass.
#[tokio::main(flavor = "current_thread")]
async fn main() -> std::process::ExitCode {
    let args = Args::parse();
    let result = async {
        let raw = std::fs::read_to_string(&args.vectors)?;
        let report = rb_eval::coding_memory::run_replay(&raw).await?;
        let json = serde_json::to_string_pretty(&report)? + "\n";
        if let Some(path) = args.output {
            std::fs::write(path, &json)?;
        }
        print!("{json}");
        Ok::<_, anyhow::Error>(())
    }
    .await;
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("coding-memory execution error: {error:#}");
            std::process::ExitCode::from(2)
        }
    }
}

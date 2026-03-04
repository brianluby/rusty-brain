use clap::Parser;
use hooks::dispatch::{Cli, Command};
use hooks::io::{fail_open, read_input, write_output};
use types::hooks::HookOutput;

fn main() {
    // Initialize tracing only if RUSTY_BRAIN_LOG is set (M-10: no stderr by default)
    if std::env::var("RUSTY_BRAIN_LOG").is_ok() {
        tracing_subscriber::fmt()
            .with_env_filter(tracing_subscriber::EnvFilter::from_env("RUSTY_BRAIN_LOG"))
            .with_writer(std::io::stderr)
            .init();
    }

    // Wrap everything in catch_unwind for fail-open safety
    let result = std::panic::catch_unwind(|| {
        let cli = match Cli::try_parse() {
            Ok(cli) => cli,
            Err(e) => {
                // Unknown subcommand or --help: write empty JSON and exit 0
                tracing::warn!("CLI parse error: {e}");
                let _ = write_output(&HookOutput::default());
                return;
            }
        };

        let input = read_input();
        let output = match input {
            Ok(input) => {
                let result = dispatch(&cli.command, &input);
                fail_open(result)
            }
            Err(e) => fail_open(Err(e)),
        };

        if write_output(&output).is_err() {
            // Last resort: write raw JSON to stdout
            print!("{{}}");
            let _ = std::io::Write::flush(&mut std::io::stdout());
        }
    });

    if result.is_err() {
        // Panic caught — write fail-open response
        print!("{{}}");
        let _ = std::io::Write::flush(&mut std::io::stdout());
    }
}

fn dispatch(
    command: &Command,
    input: &types::hooks::HookInput,
) -> Result<types::hooks::HookOutput, hooks::error::HookError> {
    match command {
        Command::SessionStart => hooks::session_start::handle_session_start(input),
        Command::PostToolUse => hooks::post_tool_use::handle_post_tool_use(input),
        Command::Stop => hooks::stop::handle_stop(input),
        Command::SmartInstall => hooks::smart_install::handle_smart_install(input),
    }
}

//! Offline assertion-grade capability gate. JSON goes to stdout even on gate failure.
#[tokio::main(flavor = "current_thread")]
async fn main() -> std::process::ExitCode {
    match rb_eval::assertion_precision::run_committed().await {
        Ok(report) => match serde_json::to_string_pretty(&report) {
            Ok(json) => {
                println!("{json}");
                std::process::ExitCode::from(u8::from(!report.precision_gate_passed))
            }
            Err(error) => {
                eprintln!("assertion-precision report error: {error}");
                std::process::ExitCode::from(2)
            }
        },
        Err(error) => {
            eprintln!("assertion-precision execution error: {error:#}");
            std::process::ExitCode::from(2)
        }
    }
}

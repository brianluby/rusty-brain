//! Per-agent installer implementations.

pub mod codex;
pub mod copilot;
pub mod gemini;
pub mod opencode;

use super::AgentInstaller;

/// Factory function for the `OpenCode` installer.
#[must_use]
pub fn opencode_installer() -> Box<dyn AgentInstaller> {
    Box::new(opencode::OpenCodeInstaller)
}

/// Factory function for the `Copilot` installer.
#[must_use]
pub fn copilot_installer() -> Box<dyn AgentInstaller> {
    Box::new(copilot::CopilotInstaller)
}

/// Factory function for the `Codex` installer.
#[must_use]
pub fn codex_installer() -> Box<dyn AgentInstaller> {
    Box::new(codex::CodexInstaller)
}

/// Factory function for the `Gemini` installer.
#[must_use]
pub fn gemini_installer() -> Box<dyn AgentInstaller> {
    Box::new(gemini::GeminiInstaller)
}

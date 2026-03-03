// Contract definitions for the compression crate.
// These are the public API types and function signatures that implementation must satisfy.
// This file is a design artifact — NOT compiled code. It defines the contract.

// ============================================================
// config.rs — Compression configuration
// ============================================================

/// Configuration controlling the compression pipeline's behavior.
///
/// All fields have sensible defaults via the `Default` impl.
/// Use `validate()` after construction with custom values to check invariants.
#[derive(Debug, Clone, PartialEq)]
pub struct CompressionConfig {
    /// Character count above which compression is triggered.
    /// Inputs at or below this threshold are returned unchanged.
    /// Default: 3,000.
    pub compression_threshold: usize,

    /// Maximum character count for compressed output.
    /// The final truncation pass guarantees this limit.
    /// Default: 2,000 (~500 tokens).
    pub target_budget: usize,
}

// Default: CompressionConfig { compression_threshold: 3_000, target_budget: 2_000 }
// Validate: target_budget > 0, compression_threshold > 0, target_budget < compression_threshold

// ============================================================
// types.rs — Core types
// ============================================================

/// The tool type, determined by case-insensitive name matching.
///
/// `From<&str>` converts tool names case-insensitively:
/// "read", "Read", "READ" all become `ToolType::Read`.
#[derive(Debug, Clone, PartialEq)]
pub enum ToolType {
    Read,
    Bash,
    Grep,
    Glob,
    Edit,
    Write,
    /// Any tool name not matching the known variants.
    Other(String),
}

// From<&str> implementation: match on tool_name.to_ascii_lowercase()

/// Result of a compression operation.
///
/// This type is always returned — the compression API is infallible.
/// When `compression_applied` is `false`, `text` contains the original input unchanged.
#[derive(Debug, Clone, PartialEq)]
pub struct CompressedResult {
    /// The (possibly compressed) output text.
    /// Guaranteed: text.chars().count() <= config.target_budget when compression_applied is true.
    pub text: String,

    /// Whether compression was actually performed.
    /// `false` when: input is empty, whitespace-only, or below the threshold.
    pub compression_applied: bool,

    /// Character count of the original input (via .chars().count()).
    pub original_size: usize,

    /// Compression diagnostics. Present only when compression_applied is true.
    pub statistics: Option<CompressionStatistics>,
}

/// Diagnostic data about a compression operation.
///
/// `PartialEq` uses epsilon comparison (1e-9) for `f64` fields (`ratio`, `percentage_saved`)
/// to avoid exact floating-point equality pitfalls. `chars_saved` is compared exactly.
#[derive(Debug, Clone)]
pub struct CompressionStatistics {
    /// Compression ratio: original_size / compressed_size.
    /// Always >= 1.0 when present.
    pub ratio: f64,

    /// Number of characters removed: original_size - compressed_size.
    pub chars_saved: usize,

    /// Percentage of original removed: (chars_saved / original_size) * 100.0.
    /// Range: 0.0–100.0.
    pub percentage_saved: f64,
}

const EPS: f64 = 1e-9;

impl PartialEq for CompressionStatistics {
    fn eq(&self, other: &Self) -> bool {
        self.chars_saved == other.chars_saved
            && (self.ratio - other.ratio).abs() <= EPS
            && (self.percentage_saved - other.percentage_saved).abs() <= EPS
    }
}

// ============================================================
// lib.rs — Public API entry point
// ============================================================

/// Compress a tool output according to its tool type.
///
/// This is the primary entry point for the compression pipeline.
/// It is infallible for valid configs: it never returns an error, and in
/// release builds it never panics. In debug builds, a `debug_assert!`
/// panics if `config` fails validation (see `CompressionConfig::validate()`).
///
/// # Behavior
///
/// 1. Empty or whitespace-only input → returned unchanged (compression_applied: false)
/// 2. Input at or below config.compression_threshold → returned unchanged
/// 3. Dispatch to specialized compressor by tool_name (case-insensitive)
/// 4. On compressor failure → fall back to generic compressor, log warning
/// 5. Final truncation enforces config.target_budget
/// 6. Build CompressedResult with statistics
///
/// # Arguments
///
/// * `config` — Compression configuration (thresholds, budget)
/// * `tool_name` — Name of the tool that produced the output (e.g., "Read", "Bash")
/// * `output` — Raw text output from the tool invocation
/// * `input_context` — Optional context string (file path for Read, command for Bash, etc.)
pub fn compress(
    config: &CompressionConfig,
    tool_name: &str,
    output: &str,
    input_context: Option<&str>,
) -> CompressedResult;

// ============================================================
// truncate.rs — Budget enforcement
// ============================================================

/// Enforce the character budget on a string.
///
/// If `text.chars().count() <= budget`, returns text unchanged.
/// Otherwise, truncates from the end preserving the head, and appends
/// a `[...truncated to N chars]` marker. The marker itself counts
/// toward the budget.
///
/// # Invariant
///
/// Return value satisfies: result.chars().count() <= budget
pub fn enforce_budget(text: &str, budget: usize) -> String;

// ============================================================
// Per-compressor module contracts
// ============================================================

// Each compressor module exposes:
//
//   pub fn compress(
//       config: &CompressionConfig,
//       output: &str,
//       input_context: Option<&str>,
//   ) -> String;
//
// Exception: edit.rs accepts an additional `is_write: bool` parameter
// to distinguish Edit ("Changes applied") from Write ("File created") output:
//
//   pub fn compress(
//       config: &CompressionConfig,
//       output: &str,
//       input_context: Option<&str>,
//       is_write: bool,
//   ) -> String;
//
// Returns compressed text (not yet budget-enforced).
// The dispatcher calls enforce_budget() after the compressor returns.
//
// Modules: read, bash, grep, glob, edit, generic

// ============================================================
// lang.rs — Language construct extraction
// ============================================================

/// Supported programming languages for construct extraction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Language {
    JavaScript, // Also covers TypeScript
    Python,
    Rust,
    Unknown,
}

/// Detect the programming language from a file path or content heuristics.
pub fn detect_language(file_path: Option<&str>, content: &str) -> Language;

/// Extract language-specific constructs from source code.
///
/// Returns a vector of extracted lines (imports, exports, function signatures,
/// class/struct declarations, error markers). Order matches source order.
/// If no constructs found, returns an empty vector.
pub fn extract_constructs(content: &str, language: Language) -> Vec<String>;

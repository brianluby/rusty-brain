//! Read tool compressor — extracts language-specific constructs.

use std::fmt::Write;

use crate::config::CompressionConfig;
use crate::generic;
use crate::lang::{detect_language, extract_constructs};

/// Compress file read output by extracting language constructs.
///
/// Detects language from `input_context` file path, extracts structural
/// constructs (imports, exports, functions, classes, error markers), and
/// produces a summary. Falls back to generic compressor if no constructs found.
///
/// Note: does not call `enforce_budget()` directly — the dispatcher in `lib.rs`
/// applies budget enforcement as the final pass after all compressors.
pub fn compress(config: &CompressionConfig, output: &str, input_context: Option<&str>) -> String {
    let language = detect_language(input_context, output);
    let constructs = extract_constructs(output, language);

    if constructs.is_empty() {
        return generic::compress(config, output, input_context);
    }

    let mut result = String::new();

    if let Some(path) = input_context {
        let _ = writeln!(result, "[File: {path}]");
    }

    let line_count = output.lines().count();
    let _ = write!(result, "[{line_count} lines, language: {language:?}]\n\n");

    for construct in &constructs {
        result.push_str(construct);
        result.push('\n');
    }

    result
}

#[cfg(test)]
mod tests {
    use crate::{CompressionConfig, compress as dispatch};

    use super::*;

    fn large_js_file() -> String {
        let mut content = String::new();
        content.push_str("import React from 'react';\n");
        content.push_str("import { useState, useEffect } from 'react';\n");
        content.push_str("const axios = require('axios');\n\n");
        content.push_str("export default function App() {\n");
        content.push_str("  // lots of component code...\n");
        for i in 0..200 {
            content.push_str(&format!("  const value{i} = computeValue({i});\n"));
        }
        content.push_str("}\n\n");
        content.push_str("export class DataService {\n");
        for i in 0..100 {
            content.push_str(&format!("  method{i}() {{ return {i}; }}\n"));
        }
        content.push_str("}\n\n");
        content.push_str("// TODO: add error handling\n");
        content.push_str("async function fetchData() {\n");
        for _ in 0..50 {
            content.push_str("  await doSomething();\n");
        }
        content.push_str("}\n");
        content
    }

    #[test]
    fn large_js_file_within_budget() {
        let config = CompressionConfig::default();
        let input = large_js_file();
        assert!(input.chars().count() > config.compression_threshold);

        let result = dispatch(&config, "Read", &input, Some("app.js"));
        assert!(result.compression_applied);
        assert!(result.text.chars().count() <= config.target_budget);
    }

    #[test]
    fn preserves_imports_and_signatures() {
        let config = CompressionConfig::default();
        let input = large_js_file();
        let result = compress(&config, &input, Some("app.js"));
        assert!(result.contains("import React"));
        assert!(result.contains("import { useState"));
        assert!(result.contains("export default function App"));
        assert!(result.contains("export class DataService"));
        assert!(result.contains("TODO"));
    }

    #[test]
    fn no_constructs_falls_to_generic() {
        let config = CompressionConfig::default();
        let input = "x".repeat(5_000);
        let result = compress(&config, &input, Some("data.bin"));
        // Generic compressor does head/tail
        assert!(result.contains("lines omitted") || result == input);
    }

    #[test]
    fn file_path_in_header() {
        let config = CompressionConfig::default();
        let input = "use std::io;\nfn main() {}\n".repeat(200);
        let result = compress(&config, &input, Some("src/main.rs"));
        assert!(result.contains("[File: src/main.rs]"));
    }

    #[test]
    fn python_file_extraction() {
        let config = CompressionConfig::default();
        let mut input = String::new();
        input.push_str("import os\n");
        input.push_str("from pathlib import Path\n\n");
        input.push_str("class Config:\n");
        input.push_str("    def __init__(self):\n");
        for i in 0..200 {
            input.push_str(&format!("        self.val{i} = {i}\n"));
        }
        input.push_str("\ndef main():\n    pass\n");

        let result = compress(&config, &input, Some("config.py"));
        assert!(result.contains("import os"));
        assert!(result.contains("from pathlib"));
        assert!(result.contains("class Config"));
        assert!(result.contains("def main"));
    }

    #[test]
    fn rust_file_extraction() {
        let config = CompressionConfig::default();
        let mut input = String::new();
        input.push_str("use std::collections::HashMap;\n");
        input.push_str("mod utils;\n\n");
        input.push_str("pub struct Config {\n");
        for i in 0..200 {
            input.push_str(&format!("    field{i}: String,\n"));
        }
        input.push_str("}\n\n");
        input.push_str("impl Config {\n");
        input.push_str("    pub fn new() -> Self { todo!() }\n");
        input.push_str("}\n");

        let result = compress(&config, &input, Some("src/config.rs"));
        assert!(result.contains("use std::collections"));
        assert!(result.contains("mod utils"));
        assert!(result.contains("pub struct Config"));
        assert!(result.contains("impl Config"));
    }
}

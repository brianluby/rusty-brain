//! Language detection and construct extraction.

use std::sync::LazyLock;

use regex::Regex;

/// Supported programming languages for construct extraction.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Language {
    /// Also covers TypeScript.
    JavaScript,
    Python,
    Rust,
    Unknown,
}

/// Detect the programming language from a file path or content heuristics.
pub(crate) fn detect_language(file_path: Option<&str>, content: &str) -> Language {
    if let Some(path) = file_path {
        if let Some((_name, ext)) = path.rsplit_once('.') {
            match ext.to_ascii_lowercase().as_str() {
                "js" | "jsx" | "ts" | "tsx" | "mjs" | "cjs" | "mts" | "cts" => {
                    return Language::JavaScript;
                }
                "py" | "pyw" | "pyi" => return Language::Python,
                "rs" => return Language::Rust,
                _ => {}
            }
        }
    }

    // Content-based heuristics
    if content.contains("#!/usr/bin/env python")
        || content.contains("#!/usr/bin/python")
        || content.contains("def __init__(self")
        || content.contains("from __future__")
    {
        return Language::Python;
    }
    if content.contains("fn main()")
        || content.contains("pub fn ")
        || content.contains("use std::")
        || content.contains("#[derive(")
    {
        return Language::Rust;
    }
    if content.contains("import React")
        || content.contains("require(")
        || content.contains("module.exports")
        || content.contains("export default")
    {
        return Language::JavaScript;
    }

    Language::Unknown
}

// --- Regex patterns ---

static JS_IMPORT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:import\s+|const\s+\w+\s*=\s*require\()").expect("BUG: invalid regex literal")
});
static JS_EXPORT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:export\s+(?:default\s+)?(?:function|class|const|let|var|interface|type|enum|async\s+function)|export\s*\{|module\.exports)").expect("BUG: invalid regex literal")
});
static JS_FUNCTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:(?:async\s+)?function\s+\w+|(?:const|let|var)\s+\w+\s*=\s*(?:async\s+)?\()")
        .expect("BUG: invalid regex literal")
});
static JS_CLASS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:class|interface)\s+\w+").expect("BUG: invalid regex literal")
});

static PY_IMPORT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:import\s+|from\s+\S+\s+import\s+)").expect("BUG: invalid regex literal")
});
static PY_FUNCTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:async\s+)?def\s+\w+").expect("BUG: invalid regex literal"));
static PY_CLASS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^class\s+\w+").expect("BUG: invalid regex literal"));

static RS_IMPORT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:pub\s+)?(?:use|mod)\s+").expect("BUG: invalid regex literal")
});
static RS_FUNCTION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:pub(?:\(crate\))?\s+)?(?:async\s+)?fn\s+\w+")
        .expect("BUG: invalid regex literal")
});
static RS_STRUCT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:pub(?:\(crate\))?\s+)?(?:struct|enum|trait)\s+\w+")
        .expect("BUG: invalid regex literal")
});
static RS_IMPL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^impl(?:<[^>]*>)?\s+").expect("BUG: invalid regex literal"));

static ERROR_MARKER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(?:TODO|FIXME|HACK|XXX|BUG)\b").expect("BUG: invalid regex literal")
});

/// Extract language-specific constructs from source code.
///
/// Returns a vector of extracted lines (imports, exports, function signatures,
/// class/struct declarations, error markers). Order matches source order.
/// If no constructs found, returns an empty vector.
pub(crate) fn extract_constructs(content: &str, language: Language) -> Vec<String> {
    if language == Language::Unknown {
        return Vec::new();
    }

    let mut constructs = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let is_match = match language {
            Language::JavaScript => {
                JS_IMPORT.is_match(trimmed)
                    || JS_EXPORT.is_match(trimmed)
                    || JS_FUNCTION.is_match(trimmed)
                    || JS_CLASS.is_match(trimmed)
                    || ERROR_MARKER.is_match(trimmed)
            }
            Language::Python => {
                PY_IMPORT.is_match(trimmed)
                    || PY_FUNCTION.is_match(trimmed)
                    || PY_CLASS.is_match(trimmed)
                    || ERROR_MARKER.is_match(trimmed)
            }
            Language::Rust => {
                RS_IMPORT.is_match(trimmed)
                    || RS_FUNCTION.is_match(trimmed)
                    || RS_STRUCT.is_match(trimmed)
                    || RS_IMPL.is_match(trimmed)
                    || ERROR_MARKER.is_match(trimmed)
            }
            Language::Unknown => false,
        };

        if is_match && seen.insert(trimmed.to_string()) {
            constructs.push(trimmed.to_string());
        }
    }

    constructs
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- detect_language tests ---

    #[test]
    fn detect_js_extensions() {
        assert_eq!(detect_language(Some("app.js"), ""), Language::JavaScript);
        assert_eq!(detect_language(Some("app.ts"), ""), Language::JavaScript);
        assert_eq!(detect_language(Some("app.tsx"), ""), Language::JavaScript);
        assert_eq!(detect_language(Some("app.jsx"), ""), Language::JavaScript);
        assert_eq!(detect_language(Some("app.mjs"), ""), Language::JavaScript);
    }

    #[test]
    fn detect_python_extensions() {
        assert_eq!(detect_language(Some("app.py"), ""), Language::Python);
        assert_eq!(detect_language(Some("app.pyi"), ""), Language::Python);
    }

    #[test]
    fn detect_rust_extension() {
        assert_eq!(detect_language(Some("main.rs"), ""), Language::Rust);
    }

    #[test]
    fn detect_unknown_extension() {
        assert_eq!(detect_language(Some("file.txt"), ""), Language::Unknown);
    }

    #[test]
    fn detect_none_path() {
        assert_eq!(detect_language(None, ""), Language::Unknown);
    }

    #[test]
    fn detect_python_by_content() {
        let content = "#!/usr/bin/env python\nprint('hello')";
        assert_eq!(detect_language(None, content), Language::Python);
    }

    #[test]
    fn detect_rust_by_content() {
        let content = "fn main() {\n    println!(\"hello\");\n}";
        assert_eq!(detect_language(None, content), Language::Rust);
    }

    #[test]
    fn detect_js_by_content() {
        let content = "import React from 'react';\nconst App = () => {};";
        assert_eq!(detect_language(None, content), Language::JavaScript);
    }

    // --- extract_constructs tests ---

    #[test]
    fn js_imports() {
        let content = "import React from 'react';\nimport { useState } from 'react';";
        let result = extract_constructs(content, Language::JavaScript);
        assert!(result.iter().any(|s| s.contains("import React")));
        assert!(result.iter().any(|s| s.contains("import { useState }")));
    }

    #[test]
    fn js_require() {
        let content = "const fs = require('fs');";
        let result = extract_constructs(content, Language::JavaScript);
        assert!(result.iter().any(|s| s.contains("require")));
    }

    #[test]
    fn js_exports() {
        let content =
            "export default function App() {}\nexport { foo, bar };\nexport const VERSION = '1.0';";
        let result = extract_constructs(content, Language::JavaScript);
        assert!(result.iter().any(|s| s.contains("export default")));
        assert!(result.iter().any(|s| s.contains("export {")));
        assert!(result.iter().any(|s| s.contains("export const")));
    }

    #[test]
    fn js_module_exports() {
        let content = "module.exports = { foo };";
        let result = extract_constructs(content, Language::JavaScript);
        assert!(result.iter().any(|s| s.contains("module.exports")));
    }

    #[test]
    fn js_functions_and_classes() {
        let content = "function hello() {}\nclass MyComponent {}\nasync function fetchData() {}";
        let result = extract_constructs(content, Language::JavaScript);
        assert!(result.iter().any(|s| s.contains("function hello")));
        assert!(result.iter().any(|s| s.contains("class MyComponent")));
        assert!(
            result
                .iter()
                .any(|s| s.contains("async function fetchData"))
        );
    }

    #[test]
    fn js_interface() {
        let content = "interface Props {\n  name: string;\n}";
        let result = extract_constructs(content, Language::JavaScript);
        assert!(result.iter().any(|s| s.contains("interface Props")));
    }

    #[test]
    fn python_imports_and_functions() {
        let content = "import os\nfrom pathlib import Path\ndef hello():\n    pass\nasync def fetch():\n    pass\nclass MyClass:\n    pass";
        let result = extract_constructs(content, Language::Python);
        assert!(result.iter().any(|s| s.contains("import os")));
        assert!(result.iter().any(|s| s.contains("from pathlib")));
        assert!(result.iter().any(|s| s.contains("def hello")));
        assert!(result.iter().any(|s| s.contains("async def fetch")));
        assert!(result.iter().any(|s| s.contains("class MyClass")));
    }

    #[test]
    fn rust_constructs() {
        let content = "use std::io;\nmod utils;\npub fn hello() {}\nstruct Config {}\nenum State {}\ntrait Handler {}\nimpl Config {}";
        let result = extract_constructs(content, Language::Rust);
        assert!(result.iter().any(|s| s.contains("use std::io")));
        assert!(result.iter().any(|s| s.contains("mod utils")));
        assert!(result.iter().any(|s| s.contains("pub fn hello")));
        assert!(result.iter().any(|s| s.contains("struct Config")));
        assert!(result.iter().any(|s| s.contains("enum State")));
        assert!(result.iter().any(|s| s.contains("trait Handler")));
        assert!(result.iter().any(|s| s.contains("impl Config")));
    }

    #[test]
    fn error_markers() {
        let content = "fn main() {}\n// TODO: fix this\n// FIXME: broken\n// HACK: temporary";
        let result = extract_constructs(content, Language::Rust);
        assert!(result.iter().any(|s| s.contains("TODO")));
        assert!(result.iter().any(|s| s.contains("FIXME")));
        assert!(result.iter().any(|s| s.contains("HACK")));
    }

    #[test]
    fn unknown_returns_empty() {
        let content = "some random text\nwith no constructs";
        let result = extract_constructs(content, Language::Unknown);
        assert!(result.is_empty());
    }

    #[test]
    fn deduplicates_identical_lines() {
        let content = "import os\nimport os\ndef hello():\n    pass";
        let result = extract_constructs(content, Language::Python);
        let import_count = result.iter().filter(|s| s.contains("import os")).count();
        assert_eq!(import_count, 1);
    }
}

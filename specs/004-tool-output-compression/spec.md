# Feature Specification: Tool-Output Compression

**Feature Branch**: `004-tool-output-compression`
**Created**: 2026-03-01
**Status**: Draft
**Input**: User description: "Port the intelligent tool-output compression system that reduces large tool outputs to ~500 tokens, with specialized compressors for Read, Edit, Write, Bash, Grep, Glob, WebFetch and other tool types"

> **Note**: WebFetch, WebSearch, and other tool types mentioned in the original description intentionally use the generic fallback compressor (FR-005) rather than receiving specialized compressors. Specialized compression is limited to the six tool types in FR-004.

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Compress Large File Reads for Memory Storage (Priority: P1)

An AI coding agent reads a large source file (e.g., 500+ lines) during a development session. Before storing this observation in memory, the system compresses the file content down to a semantic summary — preserving imports, exports, function signatures, class names, and error patterns — so the memory system can store 20× more observations within its context budget.

**Why this priority**: File reads are the most frequent tool operation in coding sessions. Without compression, a single large file read would consume the entire memory budget, making the observation storage system impractical. This is the foundational compressor that proves the architecture works.

**Independent Test**: Can be fully tested by passing representative file contents (JavaScript, Python, Rust source files) through the compressor and verifying the output contains key structural elements within the character budget.

**Acceptance Scenarios**:

1. **Given** a 10,000-character source file containing imports, function definitions, and class declarations, **When** the system compresses this Read output, **Then** the result is ≤ 2,000 characters and contains the import statements, function signatures, and class names from the original
2. **Given** a source file shorter than 3,000 characters, **When** the system processes this Read output, **Then** the output is returned unchanged (no compression applied)
3. **Given** a source file in any supported language (JavaScript, TypeScript, Python, Rust), **When** the system compresses this Read output, **Then** language-specific constructs (imports, exports, function/class declarations) are correctly identified and preserved

---

### User Story 2 - Compress Bash Command Output (Priority: P1)

An AI coding agent runs a shell command that produces verbose output (build logs, test results, installation output). The system compresses this output by highlighting errors, warnings, and success indicators while discarding intermediate noise, so the agent's memory retains the actionable information.

**Why this priority**: Bash outputs are the second most common tool output and often the largest (build logs can exceed 50,000 characters). Compressing these is essential for practical memory storage.

**Independent Test**: Can be fully tested by passing representative bash outputs (build logs with errors, test suite results, npm install output) and verifying errors and success indicators are preserved.

**Acceptance Scenarios**:

1. **Given** a 20,000-character build log containing 3 error lines scattered among informational output, **When** the system compresses this Bash output, **Then** the result is ≤ 2,000 characters and all 3 error lines are preserved
2. **Given** bash output containing success indicators (e.g., "Build successful", "All tests passed"), **When** the system compresses this output, **Then** the success indicators appear in the compressed result
3. **Given** bash output shorter than 3,000 characters, **When** the system processes this output, **Then** it is returned unchanged

---

### User Story 3 - Route Compression by Tool Type (Priority: P1)

The system automatically identifies which tool produced a given output and routes it to the appropriate specialized compressor. If no specialized compressor exists for a tool type, a generic fallback compressor is used. This dispatching happens transparently — callers provide the tool name and output, and receive compressed results.

**Why this priority**: The dispatcher is the entry point for all compression. Without it, individual compressors cannot be used. It must exist before any integration with the hook/memory pipeline.

**Independent Test**: Can be fully tested by calling the dispatcher with each supported tool name and verifying each routes to the correct compression strategy.

**Acceptance Scenarios**:

1. **Given** a tool output from a "Read" operation, **When** the dispatcher is called, **Then** the file-read compressor is used
2. **Given** a tool output from an unknown tool type (e.g., "CustomTool"), **When** the dispatcher is called, **Then** the generic fallback compressor is used
3. **Given** any tool output, **When** the dispatcher returns a result, **Then** the result includes the compressed text, whether compression was applied, and the original size in characters

---

### User Story 4 - Compress Search Results (Priority: P2)

An AI coding agent runs Grep or Glob operations that return hundreds of matching files or lines. The system compresses these results by grouping matches by file (for Grep) or by directory (for Glob), summarizing counts, and showing only the top results — preserving the searchability of the observation while dramatically reducing size.

**Why this priority**: Search operations are common and can produce very large outputs (thousands of matches). While slightly less frequent than Read/Bash, they represent a significant portion of memory budget consumption.

**Independent Test**: Can be fully tested by passing representative search outputs and verifying grouping, counting, and truncation behavior.

**Acceptance Scenarios**:

1. **Given** grep output containing 200 matches across 40 files, **When** the system compresses this output, **Then** the result shows unique file names, match counts per file, and the top 10 individual matches, all within 2,000 characters
2. **Given** glob output listing 500 files, **When** the system compresses this output, **Then** the result groups files by directory, shows the top 5 directories with file counts, and includes sample filenames from each group
3. **Given** glob output in JSON array format, **When** the system compresses this output, **Then** the JSON is parsed correctly and files are grouped by directory

---

### User Story 5 - Compress Edit and Write Operations (Priority: P2)

When an AI coding agent edits or creates files, the system compresses the operation record to just the file path and a brief change summary, since the full diff is rarely needed in memory recall. This keeps edit/write observations lightweight while preserving which files were modified.

**Why this priority**: Edit and Write operations are always captured (regardless of output length) because they represent important state changes. Their compression is simpler than other tools but still necessary for memory efficiency.

**Independent Test**: Can be fully tested by passing representative edit/write tool outputs and verifying the file path and change summary are preserved.

**Acceptance Scenarios**:

1. **Given** an Edit tool output containing a file path and a large diff, **When** the system compresses this output, **Then** the result contains the file path, a "Changes applied" indicator, and at most the first 500 characters of the original output
2. **Given** a Write tool output for a newly created file, **When** the system compresses this output, **Then** the result contains the file path and a creation indicator

---

### User Story 6 - Generic Fallback Compression (Priority: P3)

For tool types without specialized compressors (e.g., WebFetch, WebSearch, Task, NotebookEdit), the system applies a generic head/tail truncation strategy that preserves the beginning and end of the output with a clear indicator of omitted content in the middle.

**Why this priority**: The fallback ensures no tool output goes uncompressed, even as new tools are added. It provides reasonable compression without requiring per-tool customization.

**Independent Test**: Can be fully tested by passing large text blocks and verifying head/tail preservation and omission indicators.

**Acceptance Scenarios**:

1. **Given** a 15,000-character output from an unsupported tool type, **When** the system compresses this output, **Then** the result contains the first 15 lines and last 10 lines of the original, with a `[...N lines omitted...]` indicator between them
2. **Given** the compressed output, **When** examining the result, **Then** the total line count of the original is stated for context

---

### Edge Cases

- **Empty output (zero characters)**: Returned unchanged with `compression_applied: false`
- **Whitespace-only output**: Returned unchanged with `compression_applied: false`
- **File read with no recognizable language constructs** (e.g., binary file or plain text): Falls through to generic compressor (head/tail truncation)
- **Grep output with matches but no file paths** (piped input): Treated as ungrouped lines; generic compressor applied
- **Glob output neither line-delimited nor valid JSON**: Treated as plain text; generic compressor applied
- **Compressed output exceeds target budget after specialized compression**: FR-013 final truncation pass hard-truncates to budget
- **Tool name in different cases** (e.g., "read" vs "Read" vs "READ"): FR-006 case-insensitive matching applies
- **Multi-byte Unicode characters**: Counted by `char` count (not byte count) per Assumptions

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST compress tool outputs that exceed 3,000 characters
- **FR-002**: System MUST return outputs unchanged when they are 3,000 characters or fewer
- **FR-003**: System MUST produce compressed output of at most 2,000 characters (~500 tokens)
- **FR-004**: System MUST support specialized compression for these tool types: Read, Bash, Grep, Glob, Edit, Write
- **FR-005**: System MUST provide a generic fallback compressor for any unrecognized tool type
- **FR-006**: System MUST dispatch to the correct compressor based on tool name (case-insensitive matching)
- **FR-007**: System MUST return compression results that include: compressed text, whether compression was applied, and original size
- **FR-008**: System MUST extract language-specific constructs from file reads — imports, exports, function signatures, class/struct names, and error-marker comments (TODO, FIXME, HACK, XXX, BUG)
- **FR-009**: System MUST support construct extraction for at least these languages: JavaScript/TypeScript, Python, and Rust
- **FR-010**: System MUST preserve error lines and success indicators in Bash output compression
- **FR-011**: System SHOULD group grep results by file and show match counts
- **FR-012**: System SHOULD group glob results by directory and show file counts
- **FR-013**: System MUST apply a final truncation pass to ensure no compressed output exceeds the character budget, regardless of which specialized compressor produced it; truncation preserves the head and appends a `[...truncated to N chars]` marker
- **FR-014**: System SHOULD provide compression statistics (ratio, characters saved, percentage saved) for diagnostic purposes
- **FR-015**: System MUST accept an optional `input_context: Option<String>` alongside tool output to enrich compressed output when available (file path for Read, command for Bash, query for Grep/Glob)
- **FR-016**: System MUST fall back to the generic compressor when a specialized compressor fails, logging the error for diagnostics; compression MUST NOT propagate errors to the caller
- **FR-017**: System MUST limit Edit/Write compressed output to the file path, an operation indicator, and at most the first 500 characters of the original content

### Requirement Traceability (FR ↔ PRD)

| Spec FR | PRD Req | Priority | Summary |
|---------|---------|----------|---------|
| FR-001 | M-1 | Must | Compress outputs exceeding threshold |
| FR-002 | M-2 | Must | Return unchanged when below threshold |
| FR-003 | M-3 | Must | Output ≤ target budget |
| FR-004 | M-4 | Must | Specialized compressors for 6 tool types |
| FR-005 | M-5 | Must | Generic fallback for unknown tools |
| FR-006 | M-6 | Must | Case-insensitive tool name dispatch |
| FR-007 | M-7 | Must | Result includes text, flag, original size |
| FR-008 | M-8 | Must | Extract language constructs from file reads |
| FR-009 | M-9 | Must | Support JS/TS, Python, Rust |
| FR-010 | M-10 | Must | Preserve errors/success in Bash output |
| FR-011 | S-1 | Should | Group grep by file with counts |
| FR-012 | S-2 | Should | Group glob by directory with counts |
| FR-013 | M-11 | Must | Final truncation pass with marker |
| FR-014 | S-3 | Should | Compression statistics |
| FR-015 | S-4 | Should | Optional input_context |
| FR-016 | M-13 | Must | Fallback on failure, no error propagation |
| FR-017 | M-14 | Must | Edit/Write 500-char content preview limit |
| — | M-12 | Must | CompressionConfig struct (spec: Key Entities + Clarifications) |
| — | S-5 | Should | Empty/whitespace pass-through (spec: Edge Cases) |

### Key Entities

- **Tool Output**: Raw text output from a coding tool invocation, identified by tool name and an optional `input_context` string (file path for Read, command for Bash, query for Grep/Glob, etc.); the primary input to compression
- **Compressed Result**: The output of compression — contains the compressed text, a flag indicating whether compression was applied, and the original character count
- **Compression Statistics**: Diagnostic data about a compression operation — ratio, characters saved, and percentage reduction
- **Tool Type**: An identifier for which coding tool produced the output (Read, Bash, Grep, Glob, Edit, Write, or other); determines which compressor is used
- **Compression Config**: Configuration struct holding tunable parameters — compression threshold (default 3,000 chars), target budget (default 2,000 chars), and any per-compressor overrides; passed to the dispatcher at construction time

## Clarifications

### Session 2026-03-02

- Q: Should compression thresholds (3,000-char trigger, 2,000-char budget) be configurable or hardcoded constants? → A: Configurable via a `CompressionConfig` struct with sensible defaults
- Q: What should happen when tool output is empty, whitespace-only, or contains no recognizable language constructs? → A: Pass through unchanged with `compression_applied: false`; unrecognizable content in Read falls through to generic compressor
- Q: What should happen if a specialized compressor encounters an unexpected error? → A: Fall back to generic compressor on failure; log the error for diagnostics
- Q: What data should the caller provide as tool input context alongside the tool output? → A: Single optional string field (`input_context: Option<String>`) — meaning varies by tool type (file path for Read, command for Bash, etc.)
- Q: How should the final truncation pass (FR-013) work when compressed output exceeds budget? → A: Truncate from the end, preserving the head; append `[...truncated to N chars]` marker

## Assumptions

- The compression system operates on text-only tool outputs; binary content is not expected
- Character-based token estimation (characters / 4) is a sufficient heuristic; precise tokenization is not required
- The 3,000-character compression threshold and 2,000-character target budget are configurable via `CompressionConfig` with these values as defaults, derived from production usage in the TypeScript implementation
- Language construct extraction uses pattern matching; 100% accuracy is not required — false negatives (missed constructs) are acceptable, but false positives (incorrectly identified constructs) should be minimized
- The compression crate operates synchronously and does not require async runtime
- Unicode characters are counted using Rust's `char` count (Unicode scalar values), not byte count. Note: this differs from TypeScript's `string.length`, which counts UTF-16 code units (astral-plane characters like emoji count as 2 in TypeScript but 1 in Rust `char` counting)

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Compressed outputs are at most 2,000 characters for any input, regardless of original size
- **SC-002**: Compression achieves a reduction ratio of at least 10× on inputs larger than 20,000 characters
- **SC-003**: File-read compression preserves at least 80% of import statements and function signatures present in the original (measured by count of unique constructs in the output divided by count of unique constructs in the input)
- **SC-004**: Bash output compression preserves 100% of error lines present in the original
- **SC-005**: The memory system can store at least 20× more tool observations with compression enabled versus without *(integration metric — verified during pipeline integration feature, not the compression crate)*
- **SC-006**: Compression of a typical tool output (10,000 characters) completes in under 5 milliseconds
- **SC-007**: All supported tool types (Read, Bash, Grep, Glob, Edit, Write) produce semantically meaningful compressed output that retains the most important information for future memory recall

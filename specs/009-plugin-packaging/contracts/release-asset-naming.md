# Contract: Release Asset Naming

**Feature**: 009-plugin-packaging | **Date**: 2026-03-04

## Archive Naming

```
rusty-brain-v{VERSION}-{TARGET_TRIPLE}.tar.gz
```

**Examples**:
- `rusty-brain-v0.1.0-x86_64-unknown-linux-musl.tar.gz`
- `rusty-brain-v0.1.0-aarch64-unknown-linux-musl.tar.gz`
- `rusty-brain-v0.1.0-x86_64-apple-darwin.tar.gz`
- `rusty-brain-v0.1.0-aarch64-apple-darwin.tar.gz`
- `rusty-brain-v0.1.0-x86_64-pc-windows-msvc.tar.gz`

## Checksum Sidecar Naming

```
rusty-brain-v{VERSION}-{TARGET_TRIPLE}.tar.gz.sha256
```

**Content**: Hex-encoded SHA-256 hash followed by two spaces and the archive filename.

```
a1b2c3d4...  rusty-brain-v0.1.0-x86_64-unknown-linux-musl.tar.gz
```

## Archive Internal Structure

```
rusty-brain-v{VERSION}-{TARGET_TRIPLE}/
├── rusty-brain                    # CLI binary (or rusty-brain.exe on Windows)
├── rusty-brain-hooks              # Hook handler binary (or rusty-brain-hooks.exe)
├── LICENSE
└── README.md
```

## Version Format

- Tags: `v{MAJOR}.{MINOR}.{PATCH}` (e.g., `v0.1.0`)
- Version in filenames: `v{MAJOR}.{MINOR}.{PATCH}` (includes `v` prefix)
- Version in manifests: `{MAJOR}.{MINOR}.{PATCH}` (no `v` prefix)

## GitHub Release URL Template

```
https://github.com/brianluby/rusty-brain/releases/download/v{VERSION}/rusty-brain-v{VERSION}-{TARGET_TRIPLE}.tar.gz
```

## Target Triple Enumeration

| Target Triple | OS | Architecture | Linker |
|---------------|----|-------------|--------|
| `x86_64-unknown-linux-musl` | Linux | x86_64 | musl (static) |
| `aarch64-unknown-linux-musl` | Linux | aarch64 | musl (static) |
| `x86_64-apple-darwin` | macOS | x86_64 | system |
| `aarch64-apple-darwin` | macOS | aarch64 (Apple Silicon) | system |
| `x86_64-pc-windows-msvc` | Windows | x86_64 | MSVC |

# Data Model: Agent Installs

**Feature**: 011-agent-installs | **Date**: 2026-03-05

## Entities

### AgentInfo

Represents a detected agent installation on the system.

| Field | Type | Required | Description | Validation |
|-------|------|----------|-------------|------------|
| name | String | Yes | Canonical agent name (lowercase) | Must be in allowlist: opencode, copilot, codex, gemini |
| binary_path | PathBuf | Yes | Absolute path to agent binary | Must exist on filesystem |
| version | Option\<String\> | No | Detected version string | Parsed from `--version` output; None if detection fails |

### ConfigFile

A configuration file to be written for an agent.

| Field | Type | Required | Description | Validation |
|-------|------|----------|-------------|------------|
| target_path | PathBuf | Yes | Absolute path where file should be written | Must be canonical, no `..` traversal |
| content | String | Yes | File content to write | Non-empty |
| description | String | Yes | Human-readable description of what this file does | Non-empty |

### InstallScope

Determines where configuration files are placed.

| Variant | Description | Path Resolution |
|---------|-------------|-----------------|
| Project { root: PathBuf } | Config relative to current working directory | `<root>/.opencode/`, `<root>/.copilot/`, etc. |
| Global | Config in user-level directories | `~/.config/<agent>/` (Linux), `~/Library/Application Support/<agent>/` (macOS), `%APPDATA%/<agent>/` (Windows) |

### InstallConfig

Input configuration for the install orchestrator.

| Field | Type | Required | Description | Validation |
|-------|------|----------|-------------|------------|
| agents | Option\<Vec\<String\>\> | No | Explicit agent list; None = auto-detect | Each name must be in allowlist |
| scope | InstallScope | Yes | Project or Global | Must be specified (PRD M-13) |
| json | bool | Yes | Force JSON output | -- |
| reconfigure | bool | Yes | Regenerate config files | -- |
| config_dir | Option\<PathBuf\> | No | Override config directory | Must be canonical, no traversal |

### InstallReport

Output report from the install orchestrator.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| results | Vec\<AgentInstallResult\> | Yes | Per-agent results |
| memory_store | PathBuf | Yes | Path to shared `.rusty-brain/mind.mv2` |
| scope | String | Yes | "project" or "global" |

### AgentInstallResult

Per-agent installation result.

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| agent_name | String | Yes | Canonical agent name |
| status | InstallStatus | Yes | Outcome of installation |
| config_path | Option\<PathBuf\> | No | Path to main config file (None if not installed) |
| version_detected | Option\<String\> | No | Agent version if detected |
| error | Option\<String\> | No | Error message if failed |

### InstallStatus

| Variant | Description |
|---------|-------------|
| Configured | Successfully installed/configured |
| Upgraded | Existing config upgraded (backup created) |
| Skipped | Agent found but skipped (e.g., already configured, no changes needed) |
| Failed | Installation failed (see error field) |
| NotFound | Agent not found on system |

### InstallError

| Variant | Error Code | Description |
|---------|-----------|-------------|
| AgentNotFound { agent } | E_INSTALL_AGENT_NOT_FOUND | Agent binary not found on PATH |
| PermissionDenied { path, suggestion } | E_INSTALL_PERMISSION_DENIED | Cannot write to config directory |
| UnsupportedVersion { agent, version, min_version } | E_INSTALL_UNSUPPORTED_VERSION | Agent version too old |
| ConfigCorrupted { path } | E_INSTALL_CONFIG_CORRUPTED | Existing config file unparseable |
| IoError { path, source } | E_INSTALL_IO_ERROR | Filesystem I/O failure |
| ScopeRequired | E_INSTALL_SCOPE_REQUIRED | Neither --project nor --global specified |
| InvalidAgent { agent } | E_INSTALL_INVALID_AGENT | Agent name not in allowlist |
| PathTraversal { path } | E_INSTALL_PATH_TRAVERSAL | --config-dir contains traversal sequences |

## Design Notes

### AgentPlatform Configuration (PRD entity not modeled as struct)

The PRD defines an `AgentPlatform` entity (name, binary_name, detection_method, config_dir_template, min_supported_version). This is **not** represented as a runtime struct because these values are compile-time constants embedded in each `AgentInstaller` implementation. Each installer knows its own binary name, detection method, and config templates. This avoids unnecessary indirection and keeps agent knowledge co-located with agent logic.

### InstallError to AgentInstallResult.error Mapping

`InstallError` is the internal Rust enum with rich typed fields (paths, versions, source errors). `AgentInstallResult.error` is an `Option<String>` containing the `Display` output of `InstallError` for JSON serialization. The mapping is: `result.error = Some(format!("{install_error}"))`. Error codes are embedded in the Display format via `[E_INSTALL_*]` prefixes, so consumers can parse the code from the serialized string.

## Relationships

```text
InstallConfig ---> InstallOrchestrator ---> InstallerRegistry
                                       |                     \
                                       |                      --> AgentInstaller (per agent)
                                       |                              |
                                       |                              --> detect() -> Option<AgentInfo>
                                       |                              --> generate_config() -> Vec<ConfigFile>
                                       |
                                       --> ConfigWriter
                                              |
                                              --> write(ConfigFile) -> Result
                                              --> backup(Path) -> Result
                                       |
                                       --> InstallReport
                                              |
                                              --> Vec<AgentInstallResult>
```

## State Transitions

### Agent Install Lifecycle

```text
[Not Detected] --> detect() --> [Detected: AgentInfo]
                                     |
                              generate_config() --> [Config Generated: Vec<ConfigFile>]
                                     |
                              ConfigWriter::write() --> [Configured / Upgraded]
                                     |
                              validate() --> [Validated] or [Failed]
```

### Per-Agent Status Flow

```text
Start --> detect()
  |
  +--> None --> NotFound (skip, continue)
  |
  +--> Some(AgentInfo) --> generate_config()
         |
         +--> Err --> Failed (log, continue)
         |
         +--> Ok(files) --> ConfigWriter::write()
                |
                +--> existing config? --> backup --> write --> Upgraded
                |
                +--> no existing config? --> create dirs --> write --> Configured
                |
                +--> Err --> Failed (log, continue)
```

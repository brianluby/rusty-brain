//! Client connection with daemon auto-start and bounded backoff retry.

// The auto-start allowlist is owned by rb-config so every spawner (CLI, hooks)
// forwards the identical set; adding a var there widens the leak surface and
// must fail `daemon_command_forwards_only_allowlisted_vars`. The set is
// secrets + identity + XDG/HOME plus the frozen legacy knob vars (C1):
// config-file knobs need no forwarding because the spawned daemon re-reads
// `config.toml` from disk itself.
use rb_config::spawn_forward_env;
use rb_proto::{Client, ClientIdentity};
use rb_types::{Error, Namespace, Result};
use std::collections::VecDeque;
use std::fs::{self, DirBuilder, File, OpenOptions};
use std::future::Future;
use std::io::{self, Read, Seek, SeekFrom, Write};
#[cfg(unix)]
use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::net::UnixStream;
use tokio::time::Instant;

/// Build this binary's provenance identity (W0.5): `source` is the producer
/// surface (`cli` | `mcp`); user/host are left for the daemon's whoami
/// fallback (same host, same user over the UDS).
pub fn client_identity(source: &str) -> ClientIdentity {
    ClientIdentity {
        source: Some(source.to_string()),
        // Vikunja #63: the CLI's cwd is the repo context; the daemon
        // snapshots its git state once per connection for staleness.
        cwd: std::env::current_dir()
            .ok()
            .map(|p| p.to_string_lossy().into_owned()),
        ..Default::default()
    }
}

/// Connect to the daemon at `socket_path` for `namespace`, auto-starting a
/// detached `rusty-brain serve` child if the socket is not yet accepting.
/// `identity` (see [`client_identity`]) is stamped as provenance on every
/// memory the connection writes. The startup deadline covers both socket
/// connection and the existing protocol handshake.
pub async fn connect_or_start(
    socket_path: &Path,
    db_path: &Path,
    namespace: Namespace,
    self_exe: PathBuf,
    identity: Option<ClientIdentity>,
) -> Result<Client> {
    let deadline = Instant::now() + STARTUP_DEADLINE;
    let socket = socket_path.to_path_buf();
    let spawn_socket = socket.clone();
    let spawn_db = db_path.to_path_buf();
    connect_or_start_until(
        move |deadline| {
            let socket = socket.clone();
            let namespace = namespace.clone();
            let identity = identity.clone();
            async move { connect_once(&socket, namespace, identity, deadline).await }
        },
        move || spawn_daemon(&self_exe, &spawn_socket, &spawn_db, deadline),
        deadline,
        STARTUP_BACKOFF,
    )
    .await
}

const STARTUP_DEADLINE: Duration = Duration::from_secs(5);
const STARTUP_BACKOFF: Duration = Duration::from_millis(100);
const STARTUP_LOG_LIMIT: u64 = 64 * 1024;
const STARTUP_ERROR_LIMIT: usize = 8 * 1024;
const RETAINED_STARTUP_LOGS: usize = 8;
const STARTUP_COLLECTOR_ARG: &str = "--internal-startup-stderr-collector";
static STARTUP_LOG_SEQUENCE: AtomicU64 = AtomicU64::new(0);

enum ConnectAttempt<T> {
    Ready(T),
    SocketFailure(Error),
    HandshakeFailure(Error),
    SocketTimeout,
    HandshakeTimeout,
}

#[derive(Clone, Copy)]
enum StartupPhase {
    SocketUnavailable,
    SocketConnectTimedOut,
    HandshakeIncomplete,
    HandshakeRejected,
}

async fn connect_once(
    socket_path: &Path,
    namespace: Namespace,
    identity: Option<ClientIdentity>,
    deadline: Instant,
) -> ConnectAttempt<Client> {
    let stream = match tokio::time::timeout_at(deadline, UnixStream::connect(socket_path)).await {
        Ok(Ok(stream)) => stream,
        Ok(Err(error)) => return ConnectAttempt::SocketFailure(Error::from_io(&error)),
        Err(_) => return ConnectAttempt::SocketTimeout,
    };
    match tokio::time::timeout_at(deadline, Client::handshake(stream, namespace, identity)).await {
        Ok(Ok(client)) => ConnectAttempt::Ready(client),
        Ok(Err(error)) => ConnectAttempt::HandshakeFailure(error),
        Err(_) => ConnectAttempt::HandshakeTimeout,
    }
}

async fn connect_or_start_until<C, F, T, S>(
    mut connect: C,
    spawn: S,
    deadline: Instant,
    backoff: Duration,
) -> Result<T>
where
    C: FnMut(Instant) -> F,
    F: Future<Output = ConnectAttempt<T>>,
    S: FnOnce() -> Result<ManagedDaemon>,
{
    match connect(deadline).await {
        ConnectAttempt::Ready(client) => return Ok(client),
        ConnectAttempt::SocketFailure(error) if should_auto_start(&error) => {}
        ConnectAttempt::SocketFailure(error) | ConnectAttempt::HandshakeFailure(error) => {
            return Err(no_autostart_error(error));
        }
        ConnectAttempt::SocketTimeout => {
            return Err(Error::Io(
                "socket connection timed out before readiness; auto-start not attempted".into(),
            ));
        }
        ConnectAttempt::HandshakeTimeout => {
            return Err(Error::Io(
                "socket accepted a connection but the protocol handshake did not complete \
                 before the startup deadline; auto-start not attempted"
                    .into(),
            ));
        }
    }

    if Instant::now() >= deadline {
        return Err(Error::Io(
            "daemon socket was unavailable and the startup deadline expired before auto-start"
                .into(),
        ));
    }
    let mut daemon = spawn().map_err(autostart_spawn_error)?;
    let mut phase = StartupPhase::SocketUnavailable;
    let mut saw_bound_socket = false;
    let mut child_status = None;
    let mut child_wait_error = None;
    let mut last_socket_error = None;
    let mut last_handshake_error = None;

    while Instant::now() < deadline {
        match connect(deadline).await {
            ConnectAttempt::Ready(client) => {
                daemon.detach(true);
                return Ok(client);
            }
            ConnectAttempt::SocketFailure(error) => {
                phase = StartupPhase::SocketUnavailable;
                last_socket_error = Some(error);
            }
            ConnectAttempt::SocketTimeout => {
                phase = StartupPhase::SocketConnectTimedOut;
            }
            ConnectAttempt::HandshakeFailure(error) => {
                saw_bound_socket = true;
                phase = StartupPhase::HandshakeRejected;
                last_handshake_error = Some(error);
            }
            ConnectAttempt::HandshakeTimeout => {
                saw_bound_socket = true;
                phase = StartupPhase::HandshakeIncomplete;
            }
        }

        if child_status.is_none() {
            match daemon.child.try_wait() {
                Ok(status) => child_status = status,
                Err(error) => child_wait_error = Some(error.to_string()),
            }
        }
        daemon.poll_collector();

        let wake_at = (Instant::now() + backoff).min(deadline);
        tokio::time::sleep_until(wake_at).await;
    }

    if child_status.is_none() {
        match daemon.child.try_wait() {
            Ok(status) => child_status = status,
            Err(error) => child_wait_error = Some(error.to_string()),
        }
    }
    daemon.poll_collector();
    let error = startup_failure(
        &daemon,
        phase,
        saw_bound_socket,
        child_status,
        child_wait_error.as_deref(),
        last_socket_error.as_ref(),
        last_handshake_error.as_ref(),
    );
    daemon.detach(false);
    Err(error)
}

fn no_autostart_error(error: Error) -> Error {
    match error {
        Error::IoKind { kind, message } => Error::IoKind {
            kind,
            message: format!("auto-start not attempted: {}", rb_redact::redact(&message)),
        },
        Error::Io(message) => Error::Io(format!(
            "auto-start not attempted: {}",
            rb_redact::redact(&message)
        )),
        Error::Storage(message) => Error::Storage(format!(
            "auto-start not attempted: {}",
            rb_redact::redact(&message)
        )),
        error => error,
    }
}

fn autostart_spawn_error(error: Error) -> Error {
    match error {
        Error::IoKind { kind, message } => Error::IoKind {
            kind,
            message: format!("auto-start spawn failed: {}", rb_redact::redact(&message)),
        },
        error => Error::Io(format!(
            "auto-start spawn failed: {}",
            rb_redact::redact(&error.to_string())
        )),
    }
}

fn startup_failure(
    daemon: &ManagedDaemon,
    phase: StartupPhase,
    saw_bound_socket: bool,
    child_status: Option<ExitStatus>,
    child_wait_error: Option<&str>,
    last_socket_error: Option<&Error>,
    last_handshake_error: Option<&Error>,
) -> Error {
    let child_state = child_status
        .map(|status| {
            format!(
                "child exited before readiness ({})",
                exit_description(status)
            )
        })
        .unwrap_or_else(|| "child is still running; it may still be starting".into());
    let phase = match phase {
        StartupPhase::SocketUnavailable | StartupPhase::SocketConnectTimedOut
            if saw_bound_socket && last_handshake_error.is_some() =>
        {
            "the socket accepted a connection but the protocol handshake failed"
        }
        StartupPhase::SocketUnavailable | StartupPhase::SocketConnectTimedOut
            if saw_bound_socket =>
        {
            "the socket accepted a connection but the protocol handshake was incomplete"
        }
        StartupPhase::SocketUnavailable => "no connection was accepted at the daemon socket",
        StartupPhase::SocketConnectTimedOut => "connecting to the daemon socket timed out",
        StartupPhase::HandshakeIncomplete => {
            "the socket accepted a connection but the protocol handshake was incomplete"
        }
        StartupPhase::HandshakeRejected => {
            "the socket accepted a connection but the protocol handshake failed"
        }
    };
    let mut message = format!("daemon startup deadline expired: {phase}; {child_state}");
    if let Some(error) = child_wait_error {
        message.push_str(&format!("; child status unavailable: {error}"));
    }
    if let Some(status) = daemon.collector_status {
        if status.code() != Some(0) {
            message.push_str(&format!(
                "; stderr collector exited with {} and may have an incomplete log",
                exit_description(status)
            ));
        }
    }
    if daemon.collector.is_some() {
        message.push_str(
            "; stderr collector is still draining; diagnostic log snapshot may be incomplete",
        );
    }
    if let Some(error) = last_socket_error {
        message.push_str(&format!(
            "; last socket error: {}",
            rb_redact::redact(&error.to_string())
        ));
    }
    if let Some(error) = last_handshake_error {
        message.push_str(&format!(
            "; last handshake error: {}",
            rb_redact::redact(&error.to_string())
        ));
    }
    if let Some(path) = &daemon.log_path {
        let pointer = rb_redact::redact(&path.display().to_string());
        message.push_str(&format!("; startup stderr log: {pointer}"));
        if let Some(excerpt) = read_startup_log(path) {
            if !excerpt.is_empty() {
                message.push_str("; redacted stderr tail:\n");
                message.push_str(&excerpt);
            }
        }
    } else {
        message.push_str("; private startup stderr logging was unavailable");
    }
    if matches!(last_handshake_error, Some(Error::Storage(_))) {
        Error::Storage(rb_redact::redact(&message))
    } else if let Some(Error::IoKind { kind, .. }) = last_handshake_error {
        Error::IoKind {
            kind: *kind,
            message: rb_redact::redact(&message),
        }
    } else {
        Error::Io(rb_redact::redact(&message))
    }
}

fn exit_description(status: ExitStatus) -> String {
    if let Some(code) = status.code() {
        return format!("exit code {code}");
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;
        if let Some(signal) = status.signal() {
            return format!("signal {signal}");
        }
    }
    "unknown exit status".into()
}

struct ManagedDaemon {
    child: Child,
    collector: Option<Child>,
    collector_status: Option<ExitStatus>,
    log_path: Option<PathBuf>,
}

impl ManagedDaemon {
    fn poll_collector(&mut self) {
        if let Some(mut collector) = self.collector.take() {
            match collector.try_wait() {
                Ok(Some(status)) => self.collector_status = Some(status),
                Ok(None) | Err(_) => self.collector = Some(collector),
            }
        }
    }

    /// Transfer detached processes to one waiter; dropping `Child` never kills
    /// the daemon or its stderr drainer.
    fn detach(self, remove_log: bool) {
        if remove_log {
            if let Some(path) = &self.log_path {
                let _ = fs::remove_file(path);
            }
        }
        let mut child = self.child;
        let mut collector = self.collector;
        let _ = thread::Builder::new()
            .name("rusty-brain-startup-reaper".into())
            .spawn(move || {
                let _ = child.wait();
                if let Some(collector) = &mut collector {
                    let _ = collector.wait();
                }
            });
    }
}

fn read_startup_log(path: &Path) -> Option<String> {
    let file = File::open(path).ok()?;
    let mut bytes = Vec::with_capacity(STARTUP_LOG_LIMIT as usize);
    file.take(STARTUP_LOG_LIMIT).read_to_end(&mut bytes).ok()?;
    let start = bytes.len().saturating_sub(STARTUP_ERROR_LIMIT);
    let decoded = String::from_utf8_lossy(&bytes[start..]);
    let redacted = rb_redact::redact(&decoded);
    let mut safe = String::with_capacity(redacted.len().min(STARTUP_ERROR_LIMIT));
    for character in redacted.chars() {
        if !character.is_control() || matches!(character, '\n' | '\t') {
            if safe.len().saturating_add(character.len_utf8()) > STARTUP_ERROR_LIMIT {
                break;
            }
            safe.push(character);
        }
    }
    Some(safe)
}

/// Run the hidden, standalone stderr drainer before normal CLI initialization.
#[doc(hidden)]
pub fn run_internal_startup_log_collector() -> Option<std::process::ExitCode> {
    let mut args = std::env::args_os();
    let _executable = args.next();
    let mode = args.next()?;
    if mode != std::ffi::OsStr::new(STARTUP_COLLECTOR_ARG) {
        return None;
    }
    let path = args.next().map(PathBuf::from);
    let capture_millis = args
        .next()
        .and_then(|value| value.to_str().and_then(|value| value.parse::<u64>().ok()));
    if args.next().is_some() {
        return Some(collect_startup_stderr(Path::new(""), Duration::ZERO));
    }
    let (Some(path), Some(capture_millis)) = (path, capture_millis) else {
        return Some(collect_startup_stderr(Path::new(""), Duration::ZERO));
    };
    let capture_for = Duration::from_millis(capture_millis).min(STARTUP_DEADLINE);
    Some(collect_startup_stderr(&path, capture_for))
}

fn collect_startup_stderr(path: &Path, capture_for: Duration) -> std::process::ExitCode {
    let mut log = open_collector_log(path);
    let mut failed = log.is_none();
    let mut tail = VecDeque::with_capacity(STARTUP_LOG_LIMIT as usize);
    let capture_until = std::time::Instant::now() + capture_for;
    let stdin = io::stdin();
    let mut input = stdin.lock();
    let mut buffer = [0; 8192];
    loop {
        let count = match input.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => count,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => {
                failed = true;
                break;
            }
        };
        if std::time::Instant::now() >= capture_until {
            continue;
        }
        extend_startup_tail(&mut tail, &buffer[..count]);
        let write_failed = log
            .as_mut()
            .is_some_and(|file| write_startup_tail(file, &tail).is_err());
        if write_failed {
            failed = true;
            log = None;
        }
    }
    if failed {
        std::process::ExitCode::FAILURE
    } else {
        std::process::ExitCode::SUCCESS
    }
}

fn open_collector_log(path: &Path) -> Option<File> {
    if !path
        .file_name()
        .is_some_and(|name| name.to_string_lossy().starts_with("startup-"))
        || path.extension() != Some(std::ffi::OsStr::new("log"))
    {
        return None;
    }
    let directory = path.parent()?;
    if directory.file_name() != Some(std::ffi::OsStr::new("logs")) {
        return None;
    }
    let root = directory.parent()?;
    if root.file_name() != Some(std::ffi::OsStr::new("rusty-brain")) {
        return None;
    }
    for (directory, private) in [(root, false), (directory, true)] {
        let metadata = fs::symlink_metadata(directory).ok()?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return None;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = metadata.permissions().mode() & 0o777;
            let forbidden = if private { 0o077 } else { 0o022 };
            if mode & forbidden != 0 {
                return None;
            }
        }
    }
    let metadata = fs::symlink_metadata(path).ok()?;
    if metadata.file_type().is_symlink() || !metadata.is_file() || metadata.len() != 0 {
        return None;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        if metadata.permissions().mode() & 0o077 != 0 {
            return None;
        }
    }
    OpenOptions::new().read(true).write(true).open(path).ok()
}

fn extend_startup_tail(tail: &mut VecDeque<u8>, bytes: &[u8]) {
    let limit = STARTUP_LOG_LIMIT as usize;
    if bytes.len() >= limit {
        tail.clear();
        tail.extend(bytes[bytes.len() - limit..].iter().copied());
        return;
    }
    let remove = tail.len().saturating_add(bytes.len()).saturating_sub(limit);
    drop(tail.drain(..remove));
    tail.extend(bytes.iter().copied());
}

fn write_startup_tail(file: &mut File, tail: &VecDeque<u8>) -> io::Result<()> {
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    let (first, second) = tail.as_slices();
    file.write_all(first)?;
    file.write_all(second)?;
    file.flush()
}

fn spawn_daemon(
    self_exe: &Path,
    socket_path: &Path,
    db_path: &Path,
    deadline: Instant,
) -> Result<ManagedDaemon> {
    let capture_for = deadline.saturating_duration_since(Instant::now());
    spawn_observed(
        daemon_command(self_exe, socket_path, db_path),
        self_exe,
        capture_for,
    )
}

fn spawn_observed(
    command: Command,
    self_exe: &Path,
    capture_for: Duration,
) -> Result<ManagedDaemon> {
    let directory = startup_log_directory().ok();
    spawn_observed_with_log_directory(command, self_exe, directory.as_deref(), capture_for)
}

fn spawn_observed_with_log_directory(
    mut command: Command,
    self_exe: &Path,
    directory: Option<&Path>,
    capture_for: Duration,
) -> Result<ManagedDaemon> {
    let (log_path, collector) =
        match directory.and_then(|directory| diagnostic_stderr(directory, self_exe, capture_for)) {
            Some((stderr, path, collector)) => {
                command.stderr(stderr);
                (Some(path), Some(collector))
            }
            None => {
                command.stderr(Stdio::null());
                (None, None)
            }
        };
    match command.spawn() {
        Ok(child) => Ok(ManagedDaemon {
            child,
            collector,
            collector_status: None,
            log_path,
        }),
        Err(error) => {
            if let Some(path) = log_path {
                let _ = fs::remove_file(path);
            }
            if let Some(mut collector) = collector {
                let _ = thread::Builder::new()
                    .name("rusty-brain-startup-log-reaper".into())
                    .spawn(move || {
                        let _ = collector.wait();
                    });
            }
            Err(Error::from_io(&error))
        }
    }
}

fn diagnostic_stderr(
    directory: &Path,
    self_exe: &Path,
    capture_for: Duration,
) -> Option<(Stdio, PathBuf, Child)> {
    let (path, file) = create_startup_log(directory).ok()?;
    drop(file);
    let mut collector = Command::new(self_exe);
    collector
        .arg(STARTUP_COLLECTOR_ARG)
        .arg(&path)
        .arg(capture_for.as_millis().to_string())
        .env_clear()
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        collector.process_group(0);
    }
    let mut child = match collector.spawn() {
        Ok(child) => child,
        Err(_) => {
            let _ = fs::remove_file(path);
            return None;
        }
    };
    match child.stdin.take() {
        Some(stdin) => Some((Stdio::from(stdin), path, child)),
        None => {
            let _ = thread::Builder::new()
                .name("rusty-brain-startup-log-reaper".into())
                .spawn(move || {
                    let _ = child.wait();
                });
            let _ = fs::remove_file(path);
            None
        }
    }
}

fn startup_log_directory() -> io::Result<PathBuf> {
    let cache = std::env::var_os("XDG_CACHE_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .filter(|value| !value.is_empty())
                .map(|home| PathBuf::from(home).join(".cache"))
        })
        .unwrap_or_else(std::env::temp_dir);
    let root = cache.join("rusty-brain");
    ensure_log_directory(&root, false)?;
    let logs = root.join("logs");
    ensure_log_directory(&logs, true)?;
    prune_startup_logs(&logs)?;
    Ok(logs)
}

fn ensure_log_directory(path: &Path, private: bool) -> io::Result<()> {
    let mut builder = DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    builder.mode(0o700);
    match builder.create(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(error),
    }
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "startup log directory is not a real directory",
        ));
    }
    #[cfg(unix)]
    {
        let mode = metadata.permissions().mode() & 0o777;
        let forbidden = if private { 0o077 } else { 0o022 };
        if mode & forbidden != 0 {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "startup log directory permissions are too broad",
            ));
        }
    }
    Ok(())
}

fn open_private_startup_log(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    options.mode(0o600);
    options.open(path)
}

fn create_startup_log(directory: &Path) -> io::Result<(PathBuf, File)> {
    for _ in 0..16 {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let sequence = STARTUP_LOG_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = directory.join(format!(
            "startup-{}-{timestamp}-{sequence}.log",
            std::process::id()
        ));
        match open_private_startup_log(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate a unique startup log",
    ))
}

// shortcut: concurrent launchers can exceed retention; serialize creation if a strict file cap is needed.
fn prune_startup_logs(directory: &Path) -> io::Result<()> {
    let mut logs = Vec::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if !entry.file_name().to_string_lossy().starts_with("startup-")
            || !entry.file_type()?.is_file()
        {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .unwrap_or(UNIX_EPOCH);
        logs.push((modified, entry.path()));
    }
    logs.sort_by_key(|entry| entry.0);
    let remove_count = logs
        .len()
        .saturating_sub(RETAINED_STARTUP_LOGS.saturating_sub(1));
    for (_, path) in logs.into_iter().take(remove_count) {
        let _ = fs::remove_file(path);
    }
    Ok(())
}

fn daemon_command(self_exe: &Path, socket_path: &Path, db_path: &Path) -> Command {
    daemon_command_with(self_exe, socket_path, db_path, |key| {
        std::env::var(key).ok()
    })
}

/// Pure core: build the spawn `Command` from an injected env source so tests can
/// drive the allowlist without mutating process-global env (unsound under
/// parallel `#[test]` on Rust 1.81+). The real `daemon_command` injects a
/// closure over `std::env::var`.
///
/// Security: `env_clear()` is called BEFORE any `.env()`, so the child inherits
/// nothing the parent had — only the two resolved paths plus the subset of
/// [`spawn_forward_env`] that `get_env` reports as present.
fn daemon_command_with<F>(
    self_exe: &Path,
    socket_path: &Path,
    db_path: &Path,
    get_env: F,
) -> Command
where
    F: Fn(&str) -> Option<String>,
{
    let mut cmd = Command::new(self_exe);
    cmd.arg("serve");
    // Security: never inherit the parent environment into a long-lived detached
    // daemon. Clear it, then forward ONLY what the daemon needs.
    cmd.env_clear();
    cmd.env(crate::paths::SOCKET_ENV, socket_path);
    cmd.env(crate::paths::DB_ENV, db_path);
    // Forward each allowlisted var that is actually present in the source.
    for key in spawn_forward_env() {
        if let Some(value) = get_env(key) {
            cmd.env(key, value);
        }
    }
    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::null());
    cmd.stderr(Stdio::null());
    // Detach the daemon into its own process group so that a Ctrl-C / SIGHUP
    // in the client's terminal does not propagate to the long-lived daemon.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt as _;
        cmd.process_group(0);
    }
    cmd
}

/// Classify whether a connect failure is one a freshly-spawned daemon would fix.
/// Only `NotFound` (socket file absent) and `ConnectionRefused` (no listener)
/// are transient-on-start; everything else (incl. `PermissionDenied`) is
/// permanent and must NOT trigger a spawn or further retries.
fn should_auto_start_kind(kind: Option<std::io::ErrorKind>) -> bool {
    matches!(
        kind,
        Some(std::io::ErrorKind::NotFound) | Some(std::io::ErrorKind::ConnectionRefused)
    )
}

fn should_auto_start(error: &Error) -> bool {
    should_auto_start_kind(error.io_kind())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    fn test_log() -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!(
            "rusty-brain-startup-test-{}-{}",
            std::process::id(),
            STARTUP_LOG_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        ensure_log_directory(&root, false).unwrap();
        let directory = root.join("logs");
        ensure_log_directory(&directory, true).unwrap();
        let (path, file) = create_startup_log(&directory).unwrap();
        drop(file);
        (root, path)
    }

    fn shell(script: &str) -> Child {
        Command::new("/bin/sh")
            .args(["-c", script])
            .spawn()
            .unwrap()
    }

    #[tokio::test]
    async fn ready_on_first_connection_does_not_spawn() {
        let result = connect_or_start_until(
            |_| async { ConnectAttempt::Ready(7_u32) },
            || Err(Error::Io("spawn must not run".into())),
            Instant::now() + Duration::from_secs(1),
            Duration::ZERO,
        )
        .await
        .unwrap();
        assert_eq!(result, 7);
    }

    #[tokio::test]
    async fn permanent_initial_errors_preserve_classification_without_spawning() {
        let spawned = Arc::new(AtomicUsize::new(0));
        let spawn_count = Arc::clone(&spawned);
        let error = Error::from_io(&io::Error::from(io::ErrorKind::PermissionDenied));
        let result = connect_or_start_until(
            |_| {
                let error = match &error {
                    Error::IoKind { kind, message } => Error::IoKind {
                        kind: *kind,
                        message: message.clone(),
                    },
                    _ => unreachable!(),
                };
                async move { ConnectAttempt::<()>::SocketFailure(error) }
            },
            move || {
                spawn_count.fetch_add(1, Ordering::SeqCst);
                Err(Error::Io("unexpected spawn".into()))
            },
            Instant::now() + Duration::from_secs(1),
            Duration::ZERO,
        )
        .await
        .unwrap_err();
        assert_eq!(result.io_kind(), Some(io::ErrorKind::PermissionDenied));
        assert_eq!(spawned.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn protocol_handshake_failure_is_preserved_without_autostart() {
        let spawned = Arc::new(AtomicUsize::new(0));
        let spawn_count = Arc::clone(&spawned);
        let error = connect_or_start_until(
            |_| async {
                ConnectAttempt::<()>::HandshakeFailure(Error::Storage(
                    "contract version mismatch".into(),
                ))
            },
            move || {
                spawn_count.fetch_add(1, Ordering::SeqCst);
                Err(Error::Io("unexpected spawn".into()))
            },
            Instant::now() + Duration::from_secs(1),
            Duration::ZERO,
        )
        .await
        .unwrap_err();
        assert!(matches!(&error, Error::Storage(_)));
        assert!(error.to_string().contains("auto-start not attempted"));
        assert_eq!(spawned.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn stalled_handshake_is_bounded_and_does_not_autostart() {
        let directory = tempfile::tempdir().unwrap();
        let socket = directory.path().join("stalled.sock");
        let listener = tokio::net::UnixListener::bind(&socket).unwrap();
        let accepting = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            tokio::time::sleep(Duration::from_millis(250)).await;
            drop(stream);
        });
        let spawned = Arc::new(AtomicUsize::new(0));
        let spawn_count = Arc::clone(&spawned);
        let deadline = Instant::now() + Duration::from_millis(60);
        let started = Instant::now();
        let error = connect_or_start_until(
            |deadline| connect_once(&socket, Namespace::Global, None, deadline),
            move || {
                spawn_count.fetch_add(1, Ordering::SeqCst);
                Err(Error::Io("unexpected spawn".into()))
            },
            deadline,
            Duration::ZERO,
        )
        .await
        .unwrap_err();
        accepting.abort();
        assert!(error.to_string().contains("handshake did not complete"));
        assert!(error.to_string().contains("auto-start not attempted"));
        assert!(started.elapsed() < Duration::from_millis(500));
        assert_eq!(spawned.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn spawn_failure_is_reported_distinctly() {
        let error = connect_or_start_until(
            |_| async {
                ConnectAttempt::<()>::SocketFailure(Error::from_io(&io::Error::from(
                    io::ErrorKind::NotFound,
                )))
            },
            || Err(Error::Io("fixture executable missing".into())),
            Instant::now() + Duration::from_secs(1),
            Duration::ZERO,
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("auto-start spawn failed"));
        assert!(error.to_string().contains("fixture executable missing"));
    }

    #[tokio::test]
    async fn live_child_is_reported_as_still_starting_at_deadline() {
        let error = connect_or_start_until(
            |_| async {
                ConnectAttempt::<()>::SocketFailure(Error::from_io(&io::Error::from(
                    io::ErrorKind::NotFound,
                )))
            },
            || {
                let child = Command::new("/bin/sh")
                    .args(["-c", "sleep 0.3"])
                    .spawn()
                    .map_err(|error| Error::from_io(&error))?;
                Ok(ManagedDaemon {
                    child,
                    collector: None,
                    collector_status: None,
                    log_path: None,
                })
            },
            Instant::now() + Duration::from_millis(60),
            Duration::from_millis(5),
        )
        .await
        .unwrap_err();
        assert!(error.to_string().contains("child is still running"));
        assert!(error.to_string().contains("may still be starting"));
    }

    #[tokio::test]
    async fn exited_child_with_inherited_stderr_does_not_block_the_deadline() {
        let (log_root, log_path) = test_log();
        let mut collector = Command::new("/bin/cat")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stderr = Stdio::from(collector.stdin.take().unwrap());
        let child = Command::new("/bin/sh")
            .args(["-c", "(sleep 1) & exit 7"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(stderr)
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_millis(80);
        let started = std::time::Instant::now();
        let error = connect_or_start_until(
            |_| async {
                ConnectAttempt::<()>::SocketFailure(Error::from_io(&io::Error::from(
                    io::ErrorKind::NotFound,
                )))
            },
            move || {
                Ok(ManagedDaemon {
                    child,
                    collector: Some(collector),
                    collector_status: None,
                    log_path: Some(log_path),
                })
            },
            deadline,
            Duration::from_millis(5),
        )
        .await
        .unwrap_err();
        fs::remove_dir_all(log_root).unwrap();

        assert!(started.elapsed() < Duration::from_millis(500));
        assert!(error
            .to_string()
            .contains("child exited before readiness (exit code 7)"));
        assert!(error.to_string().contains("collector is still draining"));
    }

    #[tokio::test]
    async fn valid_competing_daemon_connection_wins_after_spawned_child_exits() {
        let attempts = Arc::new(AtomicUsize::new(0));
        let connect_attempts = Arc::clone(&attempts);
        let result = connect_or_start_until(
            move |_| {
                let attempt = connect_attempts.fetch_add(1, Ordering::SeqCst);
                async move {
                    if attempt == 0 {
                        ConnectAttempt::SocketFailure(Error::from_io(&io::Error::from(
                            io::ErrorKind::NotFound,
                        )))
                    } else {
                        tokio::time::sleep(Duration::from_millis(60)).await;
                        ConnectAttempt::Ready(42_u32)
                    }
                }
            },
            || {
                let child = Command::new("/bin/sh")
                    .args(["-c", "sleep 0.01; exit 7"])
                    .spawn()
                    .map_err(|error| Error::from_io(&error))?;
                Ok(ManagedDaemon {
                    child,
                    collector: None,
                    collector_status: None,
                    log_path: None,
                })
            },
            Instant::now() + Duration::from_secs(1),
            Duration::ZERO,
        )
        .await
        .unwrap();
        assert_eq!(result, 42);
    }

    #[test]
    fn detached_child_survives_launcher_handle_drop() {
        let directory = tempfile::tempdir().unwrap();
        let marker = directory.path().join("survived");
        let child = Command::new("/bin/sh")
            .args(["-c", "sleep 0.1; printf alive > \"$1\"", "sh"])
            .arg(&marker)
            .spawn()
            .unwrap();
        ManagedDaemon {
            child,
            collector: None,
            collector_status: None,
            log_path: None,
        }
        .detach(false);

        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < deadline && !marker.exists() {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(fs::read_to_string(marker).unwrap(), "alive");
    }

    fn startup_error_for_child(script: &str, log: &[u8]) -> String {
        let child = shell(script);
        let mut child_for_wait = child;
        let status = child_for_wait.wait().unwrap();
        let (root, path) = test_log();
        fs::write(&path, log).unwrap();
        let daemon = ManagedDaemon {
            child: child_for_wait,
            collector: None,
            collector_status: None,
            log_path: Some(path),
        };
        let error = startup_failure(
            &daemon,
            StartupPhase::SocketUnavailable,
            false,
            Some(status),
            None,
            None,
            None,
        )
        .to_string();
        drop(daemon);
        fs::remove_dir_all(root).unwrap();
        error
    }

    #[test]
    fn startup_failure_reports_child_exit_and_redacted_log_cause() {
        let error = startup_error_for_child(
            "exit 7",
            b"model mismatch: fixture-other-model; api_key=sensitive-value",
        );
        assert!(error.contains("child exited before readiness (exit code 7)"));
        assert!(error.contains("fixture-other-model"));
        assert!(error.contains("redacted stderr tail"));
        assert!(error.contains("[REDACTED:credential]"));
        assert!(!error.contains("sensitive-value"));
    }

    #[cfg(unix)]
    #[test]
    fn startup_failure_reports_child_signal() {
        let error = startup_error_for_child("kill -TERM $$", b"provider configuration failed");
        assert!(error.contains("child exited before readiness (signal 15)"));
        assert!(error.contains("provider configuration failed"));
    }

    #[cfg(unix)]
    #[test]
    fn startup_log_is_private_and_keeps_the_true_bounded_tail() {
        use std::os::unix::fs::PermissionsExt as _;

        let (root, path) = test_log();
        let mut bytes = vec![b'x'; STARTUP_LOG_LIMIT as usize + 16];
        bytes[..5].copy_from_slice(b"FIRST");
        let marker = b"LAST-END";
        let end = bytes.len();
        bytes[end - marker.len()..].copy_from_slice(marker);
        let mut tail = VecDeque::with_capacity(STARTUP_LOG_LIMIT as usize);
        extend_startup_tail(&mut tail, &bytes);
        let mut file = OpenOptions::new().write(true).open(&path).unwrap();
        write_startup_tail(&mut file, &tail).unwrap();
        drop(file);

        let stored = fs::read(&path).unwrap();
        assert_eq!(stored.len(), STARTUP_LOG_LIMIT as usize);
        assert!(!stored.windows(5).any(|window| window == b"FIRST"));
        assert!(stored.ends_with(marker));
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn startup_log_creation_does_not_follow_or_overwrite_symlinks() {
        use std::os::unix::fs::symlink;

        let (root, path) = test_log();
        fs::remove_file(&path).unwrap();
        let target = root.join("target");
        fs::write(&target, b"keep").unwrap();
        let link = path.with_file_name("startup-link.log");
        symlink(&target, &link).unwrap();

        assert_eq!(
            open_private_startup_log(&link).unwrap_err().kind(),
            io::ErrorKind::AlreadyExists
        );
        assert!(open_collector_log(&link).is_none());
        assert_eq!(fs::read(&target).unwrap(), b"keep");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn auto_starts_only_for_notfound_and_connection_refused() {
        assert!(should_auto_start_kind(Some(io::ErrorKind::NotFound)));
        assert!(should_auto_start_kind(Some(
            io::ErrorKind::ConnectionRefused
        )));
        assert!(!should_auto_start_kind(Some(
            io::ErrorKind::PermissionDenied
        )));
        assert!(!should_auto_start_kind(None));
    }

    #[test]
    fn daemon_command_forwards_only_allowlisted_vars() {
        use std::collections::HashMap;

        let source: HashMap<&str, &str> = HashMap::from([
            ("VOYAGE_API_KEY", "voyage-key"),
            ("RB_EMBED_BACKEND", "local"),
            ("SECRET_SHOULD_NOT_LEAK", "secret"),
        ]);
        let socket = Path::new("/tmp/rb.sock");
        let db = Path::new("/tmp/rb.db");
        let command = daemon_command_with(Path::new("/bin/echo"), socket, db, |key| {
            source.get(key).map(|value| (*value).to_string())
        });
        let envs: HashMap<_, _> = command
            .get_envs()
            .filter_map(|(key, value)| {
                value.map(|value| (key.to_os_string(), value.to_os_string()))
            })
            .collect();
        assert_eq!(
            envs.get(std::ffi::OsStr::new(crate::paths::SOCKET_ENV)),
            Some(&socket.as_os_str().to_os_string())
        );
        assert_eq!(
            envs.get(std::ffi::OsStr::new(crate::paths::DB_ENV)),
            Some(&db.as_os_str().to_os_string())
        );
        assert_eq!(
            envs.get(std::ffi::OsStr::new("VOYAGE_API_KEY")),
            Some(&std::ffi::OsString::from("voyage-key"))
        );
        assert_eq!(
            envs.get(std::ffi::OsStr::new("RB_EMBED_BACKEND")),
            Some(&std::ffi::OsString::from("local"))
        );
        assert!(!envs.contains_key(std::ffi::OsStr::new("SECRET_SHOULD_NOT_LEAK")));
        assert_eq!(envs.len(), 4);
    }
}

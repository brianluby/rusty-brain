//! Reproducible scale/concurrency benchmark for Vikunja #57.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context};
use async_trait::async_trait;
use clap::Parser;
use rb_daemon::{
    Daemon, DaemonConfig, HttpListenerConfig, SharedEmbedder, StoreHandle, WriterQueueMetrics,
};
use rb_embed::{DeterministicProvider, EmbedKind, EmbeddingProvider};
use rb_engine::MemoryBackend;
use rb_proto::{Client, Request};
use rb_store::SqliteStore;
use rb_types::{MemoryNote, MemoryType, Namespace};
use serde::Serialize;
use tokio::sync::oneshot;
use tokio::task::{JoinHandle, JoinSet};

const DIM: usize = 8;
const MODEL: &str = "deterministic";
const INPUT_VERSION: &str = "v2-composite";

#[derive(Parser, Debug)]
#[command(about = "Bounded rusty-brain scale/load benchmark", version)]
struct Options {
    /// Comma-separated fresh corpus sizes.
    #[arg(long, default_value = "1000,10000,25000")]
    corpora: String,
    /// Timed operations per transport and corpus.
    #[arg(long, default_value_t = 50)]
    operations: usize,
    /// Concurrent hook-style remember burst size.
    #[arg(long, default_value_t = 32)]
    burst: usize,
    /// JSON report path.
    #[arg(long, default_value = "target/scale-benchmark.json")]
    output: PathBuf,
}

#[derive(Serialize)]
struct Report {
    schema: &'static str,
    generated_at: String,
    git_sha: String,
    host: String,
    release_build_required: bool,
    operations: usize,
    burst: usize,
    queue_probe: QueueProbe,
    corpora: Vec<CorpusResult>,
    fault_matrix: Vec<FaultEvidence>,
}

#[derive(Serialize)]
struct QueueProbe {
    operations: usize,
    latency: Latency,
    queue: QueueResult,
    observed_live: u64,
    no_lost_committed_writes: bool,
}

#[derive(Serialize)]
struct CorpusResult {
    corpus_size: usize,
    seed_seconds: f64,
    uds_recall: Latency,
    http_recall: Latency,
    uds_remember: Latency,
    hook_burst: Latency,
    queue: QueueResult,
    dropped_broadcasts: u64,
    committed_writes: u64,
    observed_live: u64,
    no_lost_committed_writes: bool,
    namespace_isolation: bool,
    rss_bytes: u64,
    db_bytes: u64,
    wal_bytes_with_reader: u64,
    shutdown_checkpoint_ms_with_reader: u128,
    retry_checkpoint_ms: u128,
    wal_bytes_after_retry: u64,
    retry_checkpoint_cleared_wal: bool,
}

#[derive(Serialize)]
struct Latency {
    attempts: usize,
    successes: usize,
    errors: usize,
    error_p50_ms: f64,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    throughput_per_sec: f64,
}

#[derive(Serialize)]
struct QueueResult {
    enqueued: u64,
    saturated: u64,
    saturation_ratio: f64,
    average_wait_ms: f64,
    max_wait_ms: f64,
    capacity: usize,
}

#[derive(Serialize)]
struct FaultEvidence {
    case: &'static str,
    status: &'static str,
    detail: String,
}

struct Samples {
    started: Instant,
    durations: Vec<Duration>,
    error_durations: Vec<Duration>,
    errors: usize,
}

impl Samples {
    fn new(capacity: usize) -> Self {
        Self {
            started: Instant::now(),
            durations: Vec::with_capacity(capacity),
            error_durations: Vec::new(),
            errors: 0,
        }
    }

    fn success(&mut self, elapsed: Duration) {
        self.durations.push(elapsed);
    }

    fn error(&mut self, elapsed: Duration) {
        self.errors += 1;
        self.error_durations.push(elapsed);
    }

    fn finish(mut self) -> Latency {
        let wall = self.started.elapsed().as_secs_f64();
        self.durations.sort_unstable();
        self.error_durations.sort_unstable();
        let successes = self.durations.len();
        let attempts = successes + self.errors;
        Latency {
            attempts,
            successes,
            errors: self.errors,
            error_p50_ms: percentile_ms(&self.error_durations, 50),
            p50_ms: percentile_ms(&self.durations, 50),
            p95_ms: percentile_ms(&self.durations, 95),
            p99_ms: percentile_ms(&self.durations, 99),
            throughput_per_sec: if wall > 0.0 {
                successes as f64 / wall
            } else {
                0.0
            },
        }
    }
}

struct RunningDaemon {
    socket: PathBuf,
    http_addr: std::net::SocketAddr,
    store: StoreHandle,
    shutdown: oneshot::Sender<()>,
    task: JoinHandle<anyhow::Result<()>>,
}

impl RunningDaemon {
    async fn start(root: &Path, db: &Path) -> anyhow::Result<Self> {
        let socket = root.join("runtime").join("rb.sock");
        let config = DaemonConfig {
            socket_path: socket.clone(),
            db_path: db.to_path_buf(),
            read_pool_size: 4,
            jobs_config: rb_daemon::JobsConfig::default(),
            retention_policy: None,
            request_idle_timeout: None,
            enrich: None,
            fusion_mode: rb_daemon::FusionMode::Linear,
            http: Some(HttpListenerConfig::default()),
        };
        let daemon = Daemon::bind(config, SharedEmbedder::new(DeterministicProvider::new(DIM)))
            .await
            .context("bind benchmark daemon")?;
        let http_addr = daemon.http_addr().context("HTTP listener missing")?;
        let store = daemon.store_handle();
        let (shutdown, rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            daemon
                .run(async move {
                    let _ = rx.await;
                })
                .await
                .map_err(|error| anyhow!(error))
        });
        wait_for_socket(&socket).await?;
        Ok(Self {
            socket,
            http_addr,
            store,
            shutdown,
            task,
        })
    }

    async fn stop(self) -> anyhow::Result<Duration> {
        let started = Instant::now();
        let _ = self.shutdown.send(());
        tokio::time::timeout(Duration::from_secs(30), self.task)
            .await
            .context("daemon shutdown timed out")?
            .context("daemon task panicked")??;
        Ok(started.elapsed())
    }
}

#[tokio::main(flavor = "multi_thread", worker_threads = 8)]
async fn main() -> anyhow::Result<()> {
    let options = Options::parse();
    if cfg!(debug_assertions) {
        bail!("scale benchmark must run with `cargo run --release`");
    }
    if options.operations == 0 || options.burst == 0 {
        bail!("--operations and --burst must both be positive");
    }
    let corpus_sizes = parse_corpora(&options.corpora)?;
    let queue_probe = run_queue_probe(1_024).await?;
    let mut corpora = Vec::with_capacity(corpus_sizes.len());
    for corpus_size in corpus_sizes {
        corpora.push(run_corpus(corpus_size, options.operations, options.burst).await?);
    }

    let report = Report {
        schema: "rusty-brain-scale-v1",
        generated_at: chrono::Utc::now().to_rfc3339(),
        git_sha: command_output("git", &["rev-parse", "HEAD"]),
        host: command_output("uname", &["-a"]),
        release_build_required: true,
        operations: options.operations,
        burst: options.burst,
        queue_probe,
        corpora,
        fault_matrix: fault_evidence().await?,
    };
    if let Some(parent) = options.output.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    std::fs::write(&options.output, serde_json::to_vec_pretty(&report)?)
        .with_context(|| format!("write {}", options.output.display()))?;
    println!("{}", options.output.display());
    Ok(())
}

async fn run_corpus(size: usize, operations: usize, burst: usize) -> anyhow::Result<CorpusResult> {
    let dir = tempfile::tempdir().context("create benchmark directory")?;
    let db = dir.path().join("memory.db");
    let seed_started = Instant::now();
    seed_corpus(&db, size)?;
    let seed_seconds = seed_started.elapsed().as_secs_f64();

    let daemon = RunningDaemon::start(dir.path(), &db).await?;
    let dropped_before = rb_daemon::dropped_broadcast_count();
    let uds_recall = uds_recall(&daemon.socket, operations).await?;
    let http_recall = http_recall(daemon.http_addr, operations).await?;
    let uds_remember_result = uds_remember(&daemon.socket, operations, "interactive").await?;
    let (hook_burst, hook_committed) = hook_burst(&daemon.socket, burst).await;
    let committed_writes = uds_remember_result.successes as u64 + hook_committed;
    let stats = daemon
        .store
        .namespace_stats(
            Namespace::Project("scale".into()),
            30,
            MODEL.to_string(),
            INPUT_VERSION.to_string(),
            1,
            None,
        )
        .await
        .context("read post-load stats")?;
    let namespace_isolation = verify_namespace_isolation(&daemon.socket).await?;
    let queue = queue_result(daemon.store.writer_queue_metrics());
    let dropped_broadcasts = rb_daemon::dropped_broadcast_count() - dropped_before;
    let rss_bytes = current_rss_bytes();

    let reader = rusqlite::Connection::open(&db).context("open long-lived reader")?;
    reader.execute_batch("BEGIN")?;
    let _: i64 = reader.query_row("SELECT COUNT(*) FROM memories", [], |row| row.get(0))?;
    let _ = uds_remember(&daemon.socket, 8, "reader-pinned").await?;
    let wal_bytes_with_reader = file_size(&wal_path(&db));
    let shutdown_checkpoint = daemon.stop().await?;
    reader.execute_batch("COMMIT")?;
    drop(reader);

    let retry_started = Instant::now();
    let reopened = SqliteStore::open_with_model(&db, DIM, MODEL)?;
    reopened.checkpoint_truncate()?;
    let retry_checkpoint_ms = retry_started.elapsed().as_millis();
    drop(reopened);

    let wal_bytes_after_retry = file_size(&wal_path(&db));
    Ok(CorpusResult {
        corpus_size: size,
        seed_seconds,
        uds_recall,
        http_recall,
        uds_remember: uds_remember_result,
        hook_burst,
        queue,
        dropped_broadcasts,
        committed_writes,
        observed_live: stats.live,
        no_lost_committed_writes: stats.live == size as u64 + committed_writes,
        namespace_isolation,
        rss_bytes,
        db_bytes: file_size(&db),
        wal_bytes_with_reader,
        shutdown_checkpoint_ms_with_reader: shutdown_checkpoint.as_millis(),
        retry_checkpoint_ms,
        wal_bytes_after_retry,
        retry_checkpoint_cleared_wal: wal_bytes_after_retry == 0,
    })
}

async fn run_queue_probe(operations: usize) -> anyhow::Result<QueueProbe> {
    let dir = tempfile::tempdir()?;
    let db = dir.path().join("queue.db");
    let handle = StoreHandle::start_with_model(db, DIM, MODEL.to_string(), 4)?;
    let namespace = Namespace::Project("queue-probe".into());
    let started = Instant::now();
    let mut tasks = JoinSet::new();
    for index in 0..operations {
        let handle = handle.clone();
        let namespace = namespace.clone();
        tasks.spawn(async move {
            let op_started = Instant::now();
            let mut note = MemoryNote::new(
                namespace,
                format!("queue saturation probe {index}"),
                MemoryType::Insight,
                5,
            );
            note.embedding_model = MODEL.to_string();
            note.embedding_input_version = INPUT_VERSION.to_string();
            let result = handle.write(note, Some(vec![0.25; DIM])).await;
            (op_started.elapsed(), result)
        });
    }
    let mut durations = Vec::with_capacity(operations);
    let mut errors = 0;
    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok((duration, Ok(()))) => durations.push(duration),
            Ok((_, Err(_))) | Err(_) => errors += 1,
        }
    }
    let metrics = handle.writer_queue_metrics();
    let stats = handle
        .namespace_stats(
            namespace,
            30,
            MODEL.to_string(),
            INPUT_VERSION.to_string(),
            1,
            None,
        )
        .await?;
    handle.shutdown().await;
    let observed_live = stats.live;
    Ok(QueueProbe {
        operations,
        latency: Samples {
            started,
            durations,
            error_durations: vec![Duration::ZERO; errors],
            errors,
        }
        .finish(),
        queue: queue_result(metrics),
        observed_live,
        no_lost_committed_writes: observed_live == (operations - errors) as u64,
    })
}

fn seed_corpus(db: &Path, size: usize) -> anyhow::Result<()> {
    let store = SqliteStore::open_with_model(db, DIM, MODEL)?;
    let namespace = Namespace::Project("scale".into());
    for batch_start in (0..size).step_by(500) {
        let batch_end = (batch_start + 500).min(size);
        let mut rows = Vec::with_capacity(batch_end - batch_start);
        for index in batch_start..batch_end {
            let mut note = MemoryNote::new(
                namespace.clone(),
                format!("scale memory {index} about sqlite wal concurrency and agent recall"),
                MemoryType::Insight,
                5,
            );
            note.summary = format!("scale fixture {index}");
            note.keywords = vec![
                "sqlite".into(),
                "concurrency".into(),
                format!("bucket-{}", index % 97),
            ];
            note.embedding_model = MODEL.to_string();
            note.embedding_input_version = INPUT_VERSION.to_string();
            let mut vector = vec![0.0_f32; DIM];
            vector[index % DIM] = 1.0;
            rows.push((note, vector));
        }
        store.insert_memory_batch_for_benchmark(&rows)?;
    }
    store.checkpoint_truncate()?;
    Ok(())
}

async fn uds_recall(socket: &Path, operations: usize) -> anyhow::Result<Latency> {
    let mut client = Client::connect(socket, Namespace::Project("scale".into())).await?;
    let mut samples = Samples::new(operations);
    for index in 0..operations {
        let started = Instant::now();
        match client
            .recall(
                format!("sqlite concurrency bucket {}", index % 97),
                None,
                vec![],
                10,
            )
            .await
        {
            Ok(_) => samples.success(started.elapsed()),
            Err(_) => samples.error(started.elapsed()),
        }
    }
    Ok(samples.finish())
}

async fn uds_remember(socket: &Path, operations: usize, label: &str) -> anyhow::Result<Latency> {
    let mut client = Client::connect(socket, Namespace::Project("scale".into())).await?;
    let mut samples = Samples::new(operations);
    for index in 0..operations {
        let started = Instant::now();
        match client
            .remember(
                format!("{label} write {index} under bounded load"),
                None,
                MemoryType::Insight,
                5,
                vec!["load".into()],
                vec![],
                vec![],
                Some(0.7),
            )
            .await
        {
            Ok(_) => samples.success(started.elapsed()),
            Err(_) => samples.error(started.elapsed()),
        }
    }
    Ok(samples.finish())
}

async fn hook_burst(socket: &Path, burst: usize) -> (Latency, u64) {
    let mut tasks = JoinSet::new();
    let started = Instant::now();
    for index in 0..burst {
        let socket = socket.to_path_buf();
        tasks.spawn(async move {
            let op_started = Instant::now();
            let mut client = Client::connect(&socket, Namespace::Project("scale".into())).await?;
            client
                .remember(
                    format!("hook burst capture {index}"),
                    None,
                    MemoryType::Insight,
                    5,
                    vec!["hook".into()],
                    vec![],
                    vec![],
                    Some(0.7),
                )
                .await?;
            Ok::<Duration, rb_types::Error>(op_started.elapsed())
        });
    }
    let mut durations = Vec::with_capacity(burst);
    let mut errors = 0;
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok(Ok(duration)) => durations.push(duration),
            Ok(Err(_)) | Err(_) => errors += 1,
        }
    }
    let committed = durations.len() as u64;
    let samples = Samples {
        started,
        durations,
        error_durations: vec![Duration::ZERO; errors],
        errors,
    };
    (samples.finish(), committed)
}

async fn http_recall(addr: std::net::SocketAddr, operations: usize) -> anyhow::Result<Latency> {
    let client = reqwest::Client::builder().build()?;
    let url = format!("http://{addr}/recall");
    let mut samples = Samples::new(operations);
    for index in 0..operations {
        let request = Request::Recall {
            query: format!("sqlite concurrency bucket {}", index % 97),
            memory_type: None,
            tags: vec![],
            limit: 10,
            filter: rb_types::RecallFilter::default(),
        };
        let started = Instant::now();
        let response = client
            .post(&url)
            .header("x-rusty-brain-namespace", "project:scale")
            .json(&shortcut_json(request)?)
            .send()
            .await;
        match response {
            Ok(response) if response.status().is_success() => samples.success(started.elapsed()),
            Ok(_) | Err(_) => samples.error(started.elapsed()),
        }
    }
    Ok(samples.finish())
}

async fn verify_namespace_isolation(socket: &Path) -> anyhow::Result<bool> {
    let mut client = Client::connect(socket, Namespace::Project("foreign".into())).await?;
    let results = client
        .recall("sqlite concurrency".to_string(), None, vec![], 10)
        .await?;
    Ok(results.is_empty())
}

fn shortcut_json(request: Request) -> anyhow::Result<serde_json::Value> {
    let mut value = serde_json::to_value(request)?;
    value
        .as_object_mut()
        .context("request did not serialize as an object")?
        .remove("op");
    Ok(value)
}

fn queue_result(metrics: WriterQueueMetrics) -> QueueResult {
    let enqueued = metrics.enqueued.max(1);
    QueueResult {
        enqueued: metrics.enqueued,
        saturated: metrics.saturated,
        saturation_ratio: metrics.saturated as f64 / enqueued as f64,
        average_wait_ms: metrics.total_wait_ns as f64 / enqueued as f64 / 1_000_000.0,
        max_wait_ms: metrics.max_wait_ns as f64 / 1_000_000.0,
        capacity: metrics.capacity,
    }
}

fn percentile_ms(samples: &[Duration], percentile: usize) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let index = ((samples.len() * percentile).div_ceil(100)).saturating_sub(1);
    samples[index.min(samples.len() - 1)].as_secs_f64() * 1_000.0
}

fn parse_corpora(raw: &str) -> anyhow::Result<Vec<usize>> {
    let parsed: Vec<usize> = raw
        .split(',')
        .map(str::trim)
        .map(str::parse)
        .collect::<Result<_, _>>()
        .context("--corpora must be comma-separated positive integers")?;
    if parsed.is_empty() || parsed.contains(&0) {
        bail!("--corpora must contain at least one positive size");
    }
    Ok(parsed)
}

async fn wait_for_socket(socket: &Path) -> anyhow::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if tokio::net::UnixStream::connect(socket).await.is_ok() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    bail!("daemon socket did not become ready: {}", socket.display())
}

fn file_size(path: &Path) -> u64 {
    std::fs::metadata(path).map_or(0, |metadata| metadata.len())
}

fn wal_path(db: &Path) -> PathBuf {
    PathBuf::from(format!("{}-wal", db.display()))
}

fn current_rss_bytes() -> u64 {
    command_output("ps", &["-o", "rss=", "-p", &std::process::id().to_string()])
        .trim()
        .parse::<u64>()
        .unwrap_or(0)
        .saturating_mul(1024)
}

fn command_output(command: &str, args: &[&str]) -> String {
    std::process::Command::new(command)
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_else(|| "unavailable".to_string())
}

async fn fault_evidence() -> anyhow::Result<Vec<FaultEvidence>> {
    let provider_status = exercise_provider_timeout().await?;
    Ok(vec![
        FaultEvidence {
            case: "long_lived_reader",
            status: "executed_per_corpus",
            detail: "Reader transaction pins WAL during writes and graceful shutdown; report records WAL growth, blocked checkpoint duration, and explicit retry/truncate.".into(),
        },
        FaultEvidence {
            case: "interrupted_writes",
            status: "covered_by_existing_recovery_tests",
            detail: "Run `cargo test -p rb-daemon caught_writer_panic_isolates_and_does_not_lose_later_writes --locked`; the load harness independently verifies every acknowledged write remains visible.".into(),
        },
        FaultEvidence {
            case: "provider_timeout",
            status: "executed",
            detail: format!(
                "A 200ms embedding provider behind a 20ms HTTP request deadline returned HTTP {provider_status}; the daemon remained bounded and shut down cleanly."
            ),
        },
        FaultEvidence {
            case: "writer_death_recovery",
            status: "covered_by_existing_death_and_reopen_tests",
            detail: "Run `cargo test -p rb-daemon writer_alive_reports_liveness_and_flips_on_death --locked` and `cargo test -p rb-daemon writer_death_exits_the_accept_loop --locked`. No production kill switch is added for benchmarking.".into(),
        },
        FaultEvidence {
            case: "disk_full_low_disk",
            status: "blocked_not_executed",
            detail: "Requires a quota-limited disposable filesystem supplied by CI/operator; process-wide RLIMIT_FSIZE is intentionally not applied by this harness. Do not claim a result until that environment exists.".into(),
        },
    ])
}

struct SlowProvider;

#[async_trait]
impl EmbeddingProvider for SlowProvider {
    fn model_id(&self) -> &str {
        "scale-slow-provider"
    }

    fn dim(&self) -> usize {
        DIM
    }

    async fn embed(&self, texts: &[String], _kind: EmbedKind) -> rb_types::Result<Vec<Vec<f32>>> {
        tokio::time::sleep(Duration::from_millis(200)).await;
        Ok(texts.iter().map(|_| vec![0.25; DIM]).collect())
    }
}

async fn exercise_provider_timeout() -> anyhow::Result<reqwest::StatusCode> {
    let dir = tempfile::tempdir()?;
    let socket = dir.path().join("runtime").join("slow.sock");
    let config = DaemonConfig {
        socket_path: socket,
        db_path: dir.path().join("slow.db"),
        read_pool_size: 1,
        jobs_config: rb_daemon::JobsConfig::default(),
        retention_policy: None,
        request_idle_timeout: None,
        enrich: None,
        fusion_mode: rb_daemon::FusionMode::Linear,
        http: Some(HttpListenerConfig {
            request_timeout: Some(Duration::from_millis(20)),
            ..Default::default()
        }),
    };
    let daemon = Daemon::bind(config, SharedEmbedder::new(SlowProvider)).await?;
    let addr = daemon.http_addr().context("slow HTTP listener missing")?;
    let (shutdown, rx) = oneshot::channel();
    let task = tokio::spawn(async move {
        daemon
            .run(async move {
                let _ = rx.await;
            })
            .await
    });
    let request = Request::Remember {
        content: "provider timeout probe".into(),
        context: None,
        memory_type: MemoryType::Insight,
        importance: 5,
        keywords: vec![],
        tags: vec![],
        related_files: vec![],
        confidence: None,
        supersedes: None,
        anchors: vec![],
    };
    let response = reqwest::Client::new()
        .post(format!("http://{addr}/remember"))
        .header("x-rusty-brain-namespace", "project:timeout")
        .json(&shortcut_json(request)?)
        .send()
        .await?;
    let status = response.status();
    let _ = shutdown.send(());
    tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .context("slow-provider daemon shutdown timed out")?
        .context("slow-provider daemon task panicked")??;
    if status != reqwest::StatusCode::SERVICE_UNAVAILABLE {
        bail!("expected HTTP 503 at provider deadline, got {status}");
    }
    Ok(status)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    #[test]
    fn percentile_uses_nearest_rank() {
        let samples: Vec<_> = (1..=100).map(Duration::from_millis).collect();
        assert_eq!(percentile_ms(&samples, 50), 50.0);
        assert_eq!(percentile_ms(&samples, 95), 95.0);
        assert_eq!(percentile_ms(&samples, 99), 99.0);
    }

    #[test]
    fn parse_corpora_rejects_zero_and_accepts_standard_matrix() {
        assert_eq!(
            parse_corpora("1000,10000,25000").unwrap(),
            [1000, 10000, 25000]
        );
        assert!(parse_corpora("1000,0").is_err());
    }
}

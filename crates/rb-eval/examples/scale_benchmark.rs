//! Reproducible scale/concurrency benchmark for Vikunja #57.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use std::{fs::OpenOptions, io::Write};

use anyhow::{anyhow, bail, Context};
use async_trait::async_trait;
use clap::{Parser, ValueEnum};
use rb_daemon::{
    Daemon, DaemonConfig, HttpListenerConfig, ReadPoolMetrics, SharedEmbedder, StoreHandle,
    WriterQueueMetrics,
};
use rb_embed::{DeterministicProvider, EmbedKind, EmbeddingProvider};
use rb_engine::MemoryBackend;
use rb_mcp::DaemonProxy;
use rb_proto::{Client, Request, Response};
use rb_store::{SqliteStore, Store};
use rb_types::{MemoryNote, MemoryType, Namespace};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};
use tokio::sync::oneshot;
use tokio::task::{JoinHandle, JoinSet};

const INPUT_VERSION: &str = "v2-composite";
const DEFAULT_DIMENSION: usize = 384;
const DEFAULT_MODEL_ID: &str = "all-MiniLM-L6-v2";
const PRODUCTION_CORPORA: [usize; 3] = [1_000, 10_000, 25_000];
const MINIMUM_P99_SAMPLES: usize = 100;
const DISPOSABLE_MARKER: &str = ".rusty-brain-scale-disposable-volume";

#[derive(Parser, Debug)]
#[command(about = "Bounded rusty-brain scale/load benchmark", version)]
struct Options {
    /// Comma-separated fresh corpus sizes.
    #[arg(long, default_value = "1000,10000,25000")]
    corpora: String,
    /// Timed operations per transport and corpus.
    #[arg(long, default_value_t = MINIMUM_P99_SAMPLES)]
    operations: usize,
    /// Concurrent hook-style remember burst size.
    #[arg(long, default_value_t = MINIMUM_P99_SAMPLES)]
    burst: usize,
    /// Deadline for each measured operation, including MCP framing and I/O.
    #[arg(long, default_value_t = 30_000)]
    operation_timeout_ms: u64,
    /// Deadline for each embedding seed batch.
    #[arg(long, default_value_t = 120_000)]
    seed_batch_timeout_ms: u64,
    /// Embedding dimension; defaults to the production local model shape.
    #[arg(long, default_value_t = DEFAULT_DIMENSION)]
    dimension: usize,
    /// Model identity stamped into benchmark fixtures and DB metadata.
    #[arg(long, default_value = DEFAULT_MODEL_ID)]
    model_id: String,
    /// Embedding provider: real local model by default; fixture is smoke-only.
    #[arg(long, value_enum, default_value_t = ProviderMode::Local)]
    provider: ProviderMode,
    /// Explicit root of a dedicated, quota-limited disposable filesystem.
    #[arg(long)]
    disk_exhaustion_dir: Option<PathBuf>,
    /// Refuse disk exhaustion when the dedicated mount has more free space.
    #[arg(long, default_value_t = 512)]
    disk_exhaustion_max_mib: u64,
    /// Retained interrupted-write test evidence; the harness hashes this file.
    #[arg(long)]
    interrupted_writes_artifact: Option<PathBuf>,
    /// Retained writer-death/reopen test evidence; the harness hashes this file.
    #[arg(long)]
    writer_death_artifact: Option<PathBuf>,
    /// JSON report path.
    #[arg(long, default_value = "target/scale-benchmark.json")]
    output: PathBuf,
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum ProviderMode {
    Local,
    Fixture,
}

impl ProviderMode {
    fn description(self) -> &'static str {
        match self {
            Self::Local => "real local ONNX embeddings",
            Self::Fixture => "deterministic vectors with production-shaped dimension; smoke-only",
        }
    }
}

#[derive(Serialize)]
struct Report {
    schema: &'static str,
    generated_at: String,
    git_sha: String,
    worktree_clean: bool,
    host: String,
    embedding_dimension: usize,
    embedding_model_id: String,
    embedding_fixture: &'static str,
    representative_load_eligible: bool,
    production_envelope_eligible: bool,
    minimum_p99_samples: usize,
    operation_timeout_ms: u64,
    seed_batch_timeout_ms: u64,
    release_build_required: bool,
    operations: usize,
    burst: usize,
    queue_probe: QueueProbe,
    corpora: Vec<CorpusResult>,
    fault_matrix: Vec<FaultEvidence>,
}

#[derive(Clone)]
struct EmbeddingShape {
    dimension: usize,
    model_id: String,
}

struct FixtureProvider {
    inner: DeterministicProvider,
    model_id: String,
}

impl FixtureProvider {
    fn new(shape: &EmbeddingShape) -> Self {
        Self {
            inner: DeterministicProvider::new(shape.dimension),
            model_id: shape.model_id.clone(),
        }
    }
}

#[async_trait]
impl EmbeddingProvider for FixtureProvider {
    fn model_id(&self) -> &str {
        &self.model_id
    }

    fn dim(&self) -> usize {
        self.inner.dim()
    }

    async fn embed(&self, texts: &[String], kind: EmbedKind) -> rb_types::Result<Vec<Vec<f32>>> {
        self.inner.embed(texts, kind).await
    }
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
    pinned_reader_writes: Latency,
    mixed_workload: MixedWorkload,
    adequate_latency_samples: bool,
    queue: QueueResult,
    read_pool: ReadPoolResult,
    dropped_broadcasts: u64,
    committed_writes: u64,
    acknowledged_write_ids: usize,
    verified_acknowledged_write_ids: usize,
    all_acknowledged_write_ids_verified: bool,
    observed_live: u64,
    observed_live_after_reopen: u64,
    no_lost_committed_writes: bool,
    namespace_isolation: bool,
    verification_failures: Vec<&'static str>,
    verification_residual_blocking_possible: bool,
    rss_bytes: u64,
    db_bytes: u64,
    wal_bytes_with_reader: u64,
    shutdown_checkpoint_ms_with_reader: u128,
    retry_checkpoint_ms: u128,
    wal_bytes_after_retry: u64,
    retry_checkpoint_cleared_wal: bool,
}

#[derive(Serialize)]
struct MixedWorkload {
    operations_per_path: usize,
    paths: Vec<NamedLatency>,
    committed_writes: u64,
}

struct MixedOutcome {
    report: MixedWorkload,
    acknowledged_ids: Vec<rb_types::MemoryId>,
}

struct WritePhaseOutcome {
    latency: Latency,
    acknowledged_ids: Vec<rb_types::MemoryId>,
}

#[derive(Serialize)]
struct NamedLatency {
    path: &'static str,
    latency: Latency,
}

#[derive(Serialize)]
struct ReadPoolResult {
    acquired: u64,
    saturated: u64,
    saturation_ratio: f64,
    average_wait_ms: f64,
    max_wait_ms: f64,
    capacity: usize,
}

#[derive(Serialize)]
struct Latency {
    attempts: usize,
    successes: usize,
    errors: usize,
    timeouts: usize,
    residual_blocking_possible: bool,
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
    complete_for_full_eligibility: bool,
    detail: String,
}

#[derive(Deserialize)]
struct FaultArtifactRecord {
    schema: String,
    git_sha: String,
    command: String,
    exit_code: i32,
    tests: Vec<String>,
    output: String,
}

enum TimedOutcome<T> {
    Success { value: T, elapsed: Duration },
    Error { elapsed: Duration },
    Timeout { elapsed: Duration },
}

#[derive(Clone, Copy)]
struct AbsoluteOperationDeadline {
    started: Instant,
    expires: tokio::time::Instant,
}

impl AbsoluteOperationDeadline {
    fn new(budget: Duration) -> Self {
        Self {
            started: Instant::now(),
            expires: tokio::time::Instant::now() + budget,
        }
    }

    async fn run<T, F>(&self, future: F) -> Result<T, tokio::time::error::Elapsed>
    where
        F: Future<Output = T>,
    {
        tokio::time::timeout_at(self.expires, future).await
    }

    fn elapsed(self) -> Duration {
        self.started.elapsed()
    }
}

struct Samples {
    started: Instant,
    durations: Vec<Duration>,
    error_durations: Vec<Duration>,
    errors: usize,
    timeouts: usize,
    residual_blocking_possible: bool,
}

impl Samples {
    fn new(capacity: usize) -> Self {
        Self::new_at(capacity, Instant::now())
    }

    fn new_at(capacity: usize, started: Instant) -> Self {
        Self {
            started,
            durations: Vec::with_capacity(capacity),
            error_durations: Vec::new(),
            errors: 0,
            timeouts: 0,
            residual_blocking_possible: false,
        }
    }

    fn success(&mut self, elapsed: Duration) {
        self.durations.push(elapsed);
    }

    fn error(&mut self, elapsed: Duration) {
        self.errors += 1;
        self.error_durations.push(elapsed);
    }

    fn timeout(&mut self, elapsed: Duration, residual_blocking_possible: bool) {
        self.errors += 1;
        self.timeouts += 1;
        self.error_durations.push(elapsed);
        self.residual_blocking_possible |= residual_blocking_possible;
    }

    fn record<T>(&mut self, outcome: TimedOutcome<T>, residual_on_timeout: bool) -> Option<T> {
        match outcome {
            TimedOutcome::Success { value, elapsed } => {
                self.success(elapsed);
                Some(value)
            }
            TimedOutcome::Error { elapsed } => {
                self.error(elapsed);
                None
            }
            TimedOutcome::Timeout { elapsed } => {
                self.timeout(elapsed, residual_on_timeout);
                None
            }
        }
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
            timeouts: self.timeouts,
            residual_blocking_possible: self.residual_blocking_possible,
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

async fn timed_operation<T, F>(deadline: Duration, operation: F) -> TimedOutcome<T>
where
    F: Future<Output = anyhow::Result<T>>,
{
    let started = Instant::now();
    match tokio::time::timeout(deadline, operation).await {
        Ok(Ok(value)) => TimedOutcome::Success {
            value,
            elapsed: started.elapsed(),
        },
        Ok(Err(_)) => TimedOutcome::Error {
            elapsed: started.elapsed(),
        },
        Err(_) => TimedOutcome::Timeout {
            elapsed: started.elapsed(),
        },
    }
}

fn latency_is_adequate(latency: &Latency) -> bool {
    latency.attempts >= MINIMUM_P99_SAMPLES
        && latency.successes >= MINIMUM_P99_SAMPLES
        && latency.errors == 0
        && latency.timeouts == 0
}

struct RunningDaemon {
    socket: PathBuf,
    http_addr: std::net::SocketAddr,
    metrics_probe: std::sync::Arc<dyn Fn() -> (WriterQueueMetrics, ReadPoolMetrics) + Send + Sync>,
    shutdown: oneshot::Sender<()>,
    task: JoinHandle<anyhow::Result<()>>,
}

impl RunningDaemon {
    async fn start(root: &Path, db: &Path, embedder: SharedEmbedder) -> anyhow::Result<Self> {
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
        let daemon = Daemon::bind(config, embedder)
            .await
            .context("bind benchmark daemon")?;
        let http_addr = daemon.http_addr().context("HTTP listener missing")?;
        let metrics_probe = daemon.store_metrics_probe();
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
            metrics_probe,
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
    if options.dimension == 0 {
        bail!("--dimension must be positive");
    }
    if options.operation_timeout_ms == 0 || options.seed_batch_timeout_ms == 0 {
        bail!("operation and seed-batch timeouts must both be positive");
    }
    let operation_timeout = Duration::from_millis(options.operation_timeout_ms);
    let seed_batch_timeout = Duration::from_millis(options.seed_batch_timeout_ms);
    let embedder = build_embedder(options.provider, options.dimension, &options.model_id)?;
    let shape = EmbeddingShape {
        dimension: embedder.dim(),
        model_id: embedder.model_id().to_string(),
    };
    let corpus_sizes = parse_corpora(&options.corpora)?;
    let production_corpus_matrix = PRODUCTION_CORPORA
        .iter()
        .all(|required| corpus_sizes.contains(required));
    let queue_probe = run_queue_probe(1_024, &shape, operation_timeout).await?;
    let mut corpora = Vec::with_capacity(corpus_sizes.len());
    for corpus_size in corpus_sizes.iter().copied() {
        corpora.push(
            run_corpus(
                corpus_size,
                options.operations,
                options.burst,
                &shape,
                embedder.clone(),
                operation_timeout,
                seed_batch_timeout,
            )
            .await?,
        );
    }
    let git_sha = command_output("git", &["rev-parse", "HEAD"]);
    let worktree_clean = git_worktree_is_clean();
    let fault_matrix = fault_evidence(
        &shape,
        options.disk_exhaustion_dir.as_deref(),
        options.disk_exhaustion_max_mib,
        options.interrupted_writes_artifact.as_deref(),
        options.writer_death_artifact.as_deref(),
        &git_sha,
    )
    .await?;
    let representative_load_eligible = matches!(options.provider, ProviderMode::Local)
        && worktree_clean
        && shape.dimension == DEFAULT_DIMENSION
        && shape.model_id == DEFAULT_MODEL_ID
        && production_corpus_matrix
        && queue_probe.latency.errors == 0
        && queue_probe.no_lost_committed_writes
        && corpora.iter().all(|corpus| {
            corpus.adequate_latency_samples
                && corpus.no_lost_committed_writes
                && corpus.all_acknowledged_write_ids_verified
                && corpus.namespace_isolation
                && corpus.verification_failures.is_empty()
                && corpus.retry_checkpoint_cleared_wal
        });
    let production_envelope_eligible =
        representative_load_eligible && mandatory_fault_evidence_complete(&fault_matrix);

    let report = Report {
        schema: "rusty-brain-scale-v3",
        generated_at: chrono::Utc::now().to_rfc3339(),
        git_sha,
        worktree_clean,
        host: command_output("uname", &["-a"]),
        embedding_dimension: shape.dimension,
        embedding_model_id: shape.model_id.clone(),
        embedding_fixture: options.provider.description(),
        representative_load_eligible,
        production_envelope_eligible,
        minimum_p99_samples: MINIMUM_P99_SAMPLES,
        operation_timeout_ms: options.operation_timeout_ms,
        seed_batch_timeout_ms: options.seed_batch_timeout_ms,
        release_build_required: true,
        operations: options.operations,
        burst: options.burst,
        queue_probe,
        corpora,
        fault_matrix,
    };
    if let Some(parent) = options.output.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
    }
    std::fs::write(&options.output, serde_json::to_vec_pretty(&report)?)
        .with_context(|| format!("write {}", options.output.display()))?;
    println!("{}", options.output.display());
    Ok(())
}

fn build_embedder(
    provider: ProviderMode,
    dimension: usize,
    model_id: &str,
) -> anyhow::Result<SharedEmbedder> {
    match provider {
        ProviderMode::Fixture => Ok(SharedEmbedder::new(FixtureProvider::new(&EmbeddingShape {
            dimension,
            model_id: model_id.to_string(),
        }))),
        ProviderMode::Local => {
            #[cfg(feature = "record-local")]
            {
                let local = rb_embed::LocalProvider::load(model_id)?;
                if local.dim() != dimension {
                    bail!(
                        "local model {model_id} is {}-dimensional, but --dimension requested {dimension}",
                        local.dim()
                    );
                }
                Ok(SharedEmbedder::new(local))
            }
            #[cfg(not(feature = "record-local"))]
            {
                let _ = (dimension, model_id);
                bail!(
                    "the default local provider requires rb-eval feature `record-local`; use scripts/run-scale-benchmark.sh or pass --provider fixture for smoke-only shape tests"
                )
            }
        }
    }
}

async fn run_corpus(
    size: usize,
    operations: usize,
    burst: usize,
    shape: &EmbeddingShape,
    embedder: SharedEmbedder,
    operation_timeout: Duration,
    seed_batch_timeout: Duration,
) -> anyhow::Result<CorpusResult> {
    let dir = tempfile::tempdir().context("create benchmark directory")?;
    let db = dir.path().join("memory.db");
    let seed_started = Instant::now();
    seed_corpus(&db, size, shape, &embedder, seed_batch_timeout).await?;
    let seed_seconds = seed_started.elapsed().as_secs_f64();

    let daemon = RunningDaemon::start(dir.path(), &db, embedder).await?;
    let dropped_before = rb_daemon::dropped_broadcast_count();
    let uds_recall = uds_recall(&daemon.socket, operations, operation_timeout).await;
    let http_recall = http_recall(daemon.http_addr, operations, operation_timeout).await?;
    let uds_remember_result =
        uds_remember(&daemon.socket, operations, "interactive", operation_timeout).await;
    let hook_result = hook_burst(&daemon.socket, burst, operation_timeout).await;
    let mixed = mixed_workload(
        &daemon.socket,
        daemon.http_addr,
        operations,
        operation_timeout,
    )
    .await?;
    let WritePhaseOutcome {
        latency: uds_remember_latency,
        acknowledged_ids: mut acknowledged_write_ids,
    } = uds_remember_result;
    let WritePhaseOutcome {
        latency: hook_burst,
        acknowledged_ids: hook_ids,
    } = hook_result;
    acknowledged_write_ids.extend(hook_ids);
    acknowledged_write_ids.extend(mixed.acknowledged_ids);
    let mut verification_failures = Vec::new();
    let mut verification_residual_blocking_possible = false;
    let stats = match tokio::time::timeout(operation_timeout, async {
        let mut client =
            Client::connect(&daemon.socket, Namespace::Project("scale".into())).await?;
        let (stats, _, _) = client.stats(Some(30)).await?;
        anyhow::Ok(stats)
    })
    .await
    {
        Ok(Ok(stats)) => Some(stats),
        Ok(Err(_)) => {
            verification_failures.push("post_load_stats_error");
            None
        }
        Err(_) => {
            verification_failures.push("post_load_stats_timeout");
            verification_residual_blocking_possible = true;
            None
        }
    };
    let namespace_isolation = match tokio::time::timeout(
        operation_timeout,
        verify_namespace_isolation(&daemon.socket),
    )
    .await
    {
        Ok(Ok(isolated)) => isolated,
        Ok(Err(_)) => {
            verification_failures.push("namespace_isolation_error");
            false
        }
        Err(_) => {
            verification_failures.push("namespace_isolation_timeout");
            verification_residual_blocking_possible = true;
            false
        }
    };
    let (queue_metrics, read_metrics) = (daemon.metrics_probe)();
    let queue = queue_result(queue_metrics);
    let read_pool = read_pool_result(read_metrics);
    let dropped_broadcasts = rb_daemon::dropped_broadcast_count() - dropped_before;
    let rss_bytes = current_rss_bytes();

    let reader = open_pinned_reader(&db).context("open long-lived reader")?;
    reader.execute_batch("BEGIN")?;
    let _: i64 = reader.query_row("SELECT COUNT(*) FROM memories", [], |row| row.get(0))?;
    let pinned_result = uds_remember(
        &daemon.socket,
        operations,
        "reader-pinned",
        operation_timeout,
    )
    .await;
    let WritePhaseOutcome {
        latency: pinned_reader_writes,
        acknowledged_ids: pinned_ids,
    } = pinned_result;
    acknowledged_write_ids.extend(pinned_ids);
    let wal_bytes_with_reader = file_size(&wal_path(&db));
    let shutdown_checkpoint = daemon.stop().await?;
    reader.execute_batch("COMMIT")?;
    drop(reader);

    let retry_started = Instant::now();
    let reopen_db = db.clone();
    let reopen_dimension = shape.dimension;
    let reopen_model_id = shape.model_id.clone();
    let acknowledged_write_id_count = acknowledged_write_ids.len();
    let committed_writes = u64::try_from(acknowledged_write_id_count).unwrap_or(u64::MAX);
    let expected_live = size as u64 + committed_writes;
    let mut reopen = tokio::task::spawn_blocking(move || -> anyhow::Result<(u64, u64, usize)> {
        let reopened =
            SqliteStore::open_with_model(&reopen_db, reopen_dimension, &reopen_model_id)?;
        reopened.checkpoint_truncate()?;
        let stats = reopened.namespace_stats(
            &Namespace::Project("scale".into()),
            30,
            &reopen_model_id,
            INPUT_VERSION,
            1,
            None,
        )?;
        let expected_namespace = Namespace::Project("scale".into());
        let mut verified_ids = 0;
        for id in &acknowledged_write_ids {
            if reopened
                .get_memory(id)?
                .is_some_and(|note| note.namespace == expected_namespace)
            {
                verified_ids += 1;
            }
        }
        drop(reopened);
        Ok((stats.live, file_size(&wal_path(&reopen_db)), verified_ids))
    });
    let (observed_live_after_reopen, wal_bytes_after_retry, verified_ids, reopen_ok) =
        match tokio::time::timeout(operation_timeout, &mut reopen).await {
            Ok(Ok(Ok((live, wal_bytes, verified_ids)))) => (live, wal_bytes, verified_ids, true),
            Ok(Ok(Err(_))) | Ok(Err(_)) => {
                verification_failures.push("reopen_checkpoint_error");
                (0, file_size(&wal_path(&db)), 0, false)
            }
            Err(_) => {
                reopen.abort();
                verification_failures.push("reopen_checkpoint_timeout");
                verification_residual_blocking_possible = true;
                (0, file_size(&wal_path(&db)), 0, false)
            }
        };
    let all_acknowledged_write_ids_verified =
        reopen_ok && verified_ids == acknowledged_write_id_count;
    if reopen_ok && !all_acknowledged_write_ids_verified {
        verification_failures.push("acknowledged_write_id_missing_or_wrong_namespace");
    }
    let retry_checkpoint_ms = retry_started.elapsed().as_millis();
    let adequate_latency_samples = latency_is_adequate(&uds_recall)
        && latency_is_adequate(&http_recall)
        && latency_is_adequate(&uds_remember_latency)
        && latency_is_adequate(&hook_burst)
        && latency_is_adequate(&pinned_reader_writes)
        && mixed
            .report
            .paths
            .iter()
            .all(|path| latency_is_adequate(&path.latency));
    Ok(CorpusResult {
        corpus_size: size,
        seed_seconds,
        uds_recall,
        http_recall,
        uds_remember: uds_remember_latency,
        hook_burst,
        pinned_reader_writes,
        mixed_workload: mixed.report,
        adequate_latency_samples,
        queue,
        read_pool,
        dropped_broadcasts,
        committed_writes,
        acknowledged_write_ids: acknowledged_write_id_count,
        verified_acknowledged_write_ids: verified_ids,
        all_acknowledged_write_ids_verified,
        observed_live: stats.as_ref().map_or(0, |stats| stats.live),
        observed_live_after_reopen,
        no_lost_committed_writes: reopen_ok && observed_live_after_reopen == expected_live,
        namespace_isolation,
        verification_failures,
        verification_residual_blocking_possible,
        rss_bytes,
        db_bytes: file_size(&db),
        wal_bytes_with_reader,
        shutdown_checkpoint_ms_with_reader: shutdown_checkpoint.as_millis(),
        retry_checkpoint_ms,
        wal_bytes_after_retry,
        retry_checkpoint_cleared_wal: reopen_ok && wal_bytes_after_retry == 0,
    })
}

async fn run_queue_probe(
    operations: usize,
    shape: &EmbeddingShape,
    operation_timeout: Duration,
) -> anyhow::Result<QueueProbe> {
    let dir = tempfile::tempdir()?;
    let db = dir.path().join("queue.db");
    let handle = StoreHandle::start_with_model(db, shape.dimension, shape.model_id.clone(), 4)?;
    let namespace = Namespace::Project("queue-probe".into());
    let phase_started = Instant::now();
    let mut samples = Samples::new_at(operations, phase_started);
    let mut tasks = JoinSet::new();
    for index in 0..operations {
        let handle = handle.clone();
        let namespace = namespace.clone();
        let model_id = shape.model_id.clone();
        let dimension = shape.dimension;
        tasks.spawn(async move {
            let mut note = MemoryNote::new(
                namespace,
                format!("queue saturation probe {index}"),
                MemoryType::Insight,
                5,
            );
            note.embedding_model = model_id;
            note.embedding_input_version = INPUT_VERSION.to_string();
            timed_operation(operation_timeout, async move {
                handle
                    .write(note, Some(vec![0.25; dimension]))
                    .await
                    .map_err(anyhow::Error::from)
            })
            .await
        });
    }
    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok(outcome) => {
                samples.record(outcome, true);
            }
            Err(_) => samples.error(Duration::ZERO),
        }
    }
    let latency = samples.finish();
    let metrics = handle.writer_queue_metrics();
    let stats = tokio::time::timeout(
        operation_timeout,
        handle.namespace_stats(
            namespace,
            30,
            shape.model_id.clone(),
            INPUT_VERSION.to_string(),
            1,
            None,
        ),
    )
    .await
    .context("queue-probe stats timed out; SQLite blocking work may remain active")??;
    tokio::time::timeout(operation_timeout, handle.shutdown())
        .await
        .context("queue-probe shutdown timed out; writer work may remain active")?;
    let observed_live = stats.live;
    let expected_live = u64::try_from(latency.successes).unwrap_or(u64::MAX);
    Ok(QueueProbe {
        operations,
        latency,
        queue: queue_result(metrics),
        observed_live,
        no_lost_committed_writes: observed_live == expected_live,
    })
}

async fn seed_corpus(
    db: &Path,
    size: usize,
    shape: &EmbeddingShape,
    embedder: &SharedEmbedder,
    seed_batch_timeout: Duration,
) -> anyhow::Result<()> {
    let namespace = Namespace::Project("scale".into());
    for batch_start in (0..size).step_by(500) {
        let batch_end = (batch_start + 500).min(size);
        let mut notes = Vec::with_capacity(batch_end - batch_start);
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
            note.embedding_model = shape.model_id.clone();
            note.embedding_input_version = INPUT_VERSION.to_string();
            notes.push(note);
        }
        let inputs: Vec<_> = notes.iter().map(rb_engine::embedding_input).collect();
        let embeddings = tokio::time::timeout(
            seed_batch_timeout,
            embedder.embed(&inputs, EmbedKind::Document),
        )
        .await
        .with_context(|| format!("seed embedding batch {batch_start}..{batch_end} timed out"))??;
        if embeddings.len() != notes.len()
            || embeddings
                .iter()
                .any(|embedding| embedding.len() != shape.dimension)
        {
            bail!("embedding provider returned the wrong batch shape");
        }
        let rows: Vec<_> = notes.into_iter().zip(embeddings).collect();
        let db = db.to_path_buf();
        let dimension = shape.dimension;
        let model_id = shape.model_id.clone();
        let mut insert = tokio::task::spawn_blocking(move || -> anyhow::Result<()> {
            let store = SqliteStore::open_with_model(&db, dimension, &model_id)?;
            store.insert_memory_batch_for_benchmark(&rows)?;
            Ok(())
        });
        match tokio::time::timeout(seed_batch_timeout, &mut insert).await {
            Ok(joined) => joined.context("seed batch worker panicked")??,
            Err(_) => {
                insert.abort();
                bail!(
                    "seed database batch {batch_start}..{batch_end} timed out; SQLite spawn_blocking work may remain active after cancellation"
                );
            }
        }
    }
    let store = SqliteStore::open_with_model(db, shape.dimension, &shape.model_id)?;
    store.checkpoint_truncate()?;
    Ok(())
}

async fn uds_recall(socket: &Path, operations: usize, deadline: Duration) -> Latency {
    let mut samples = Samples::new(operations);
    for index in 0..operations {
        let socket = socket.to_path_buf();
        let outcome = timed_operation(deadline, async move {
            let mut client = Client::connect(&socket, Namespace::Project("scale".into())).await?;
            client
                .recall(
                    format!("sqlite concurrency bucket {}", index % 97),
                    None,
                    vec![],
                    10,
                )
                .await?;
            Ok(())
        })
        .await;
        samples.record(outcome, true);
    }
    samples.finish()
}

async fn uds_remember(
    socket: &Path,
    operations: usize,
    label: &str,
    deadline: Duration,
) -> WritePhaseOutcome {
    let mut samples = Samples::new(operations);
    let mut acknowledged_ids = Vec::with_capacity(operations);
    for index in 0..operations {
        let socket = socket.to_path_buf();
        let label = label.to_string();
        let outcome = timed_operation(deadline, async move {
            let mut client = Client::connect(&socket, Namespace::Project("scale".into())).await?;
            let id = client
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
                .await?;
            Ok(id)
        })
        .await;
        if let Some(id) = samples.record(outcome, true) {
            acknowledged_ids.push(id);
        }
    }
    WritePhaseOutcome {
        latency: samples.finish(),
        acknowledged_ids,
    }
}

async fn hook_burst(socket: &Path, burst: usize, deadline: Duration) -> WritePhaseOutcome {
    let phase_started = Instant::now();
    let mut samples = Samples::new_at(burst, phase_started);
    let mut tasks = JoinSet::new();
    for index in 0..burst {
        let socket = socket.to_path_buf();
        tasks.spawn(async move {
            timed_operation(deadline, async move {
                let mut client =
                    Client::connect(&socket, Namespace::Project("scale".into())).await?;
                let id = client
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
                Ok(id)
            })
            .await
        });
    }
    let mut acknowledged_ids = Vec::with_capacity(burst);
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok(outcome) => {
                if let Some(id) = samples.record(outcome, true) {
                    acknowledged_ids.push(id);
                }
            }
            Err(_) => samples.error(Duration::ZERO),
        }
    }
    WritePhaseOutcome {
        latency: samples.finish(),
        acknowledged_ids,
    }
}

#[derive(Clone, Copy)]
enum MixedOp {
    UdsRecall,
    UdsRemember,
    HttpRecall,
    HttpRemember,
    McpRecall,
    McpRemember,
}

impl MixedOp {
    const ALL: [Self; 6] = [
        Self::UdsRecall,
        Self::UdsRemember,
        Self::HttpRecall,
        Self::HttpRemember,
        Self::McpRecall,
        Self::McpRemember,
    ];

    fn name(self) -> &'static str {
        match self {
            Self::UdsRecall => "uds_recall",
            Self::UdsRemember => "uds_remember",
            Self::HttpRecall => "http_recall",
            Self::HttpRemember => "http_remember",
            Self::McpRecall => "mcp_recall",
            Self::McpRemember => "mcp_remember",
        }
    }
}

struct McpClientProxy {
    client: Client,
}

#[async_trait]
impl DaemonProxy for McpClientProxy {
    async fn call(&mut self, request: Request) -> rb_types::Result<Response> {
        self.client.request(request).await
    }
}

#[derive(Clone, Copy)]
enum McpToolKind {
    Recall,
    Remember,
}

struct McpStdioSession {
    writer: DuplexStream,
    reader: BufReader<DuplexStream>,
    server: Option<JoinHandle<rb_types::Result<()>>>,
    next_id: u64,
}

impl McpStdioSession {
    async fn start(socket: &Path) -> anyhow::Result<Self> {
        let client = Client::connect(socket, Namespace::Project("scale".into())).await?;
        Self::start_with_proxy(McpClientProxy { client }).await
    }

    async fn start_with_proxy<P>(proxy: P) -> anyhow::Result<Self>
    where
        P: DaemonProxy + 'static,
    {
        let (writer, server_reader) = tokio::io::duplex(64 * 1024);
        let (server_writer, reader) = tokio::io::duplex(64 * 1024);
        let server = tokio::spawn(async move {
            rb_mcp::serve_stdio(BufReader::new(server_reader), server_writer, proxy).await
        });
        let mut session = Self {
            writer,
            reader: BufReader::new(reader),
            server: Some(server),
            next_id: 1,
        };
        let initialize = serde_json::json!({
            "jsonrpc": "2.0",
            "id": session.next_request_id(),
            "method": "initialize",
            "params": { "protocolVersion": "2024-11-05" },
        });
        let response = session.exchange(&initialize).await?;
        validate_jsonrpc_result(&response, 1)?;
        let initialized = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
        });
        session.write_frame(&initialized).await?;
        Ok(session)
    }

    fn next_request_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    async fn call_tool(
        &mut self,
        kind: McpToolKind,
        index: usize,
    ) -> anyhow::Result<Option<rb_types::MemoryId>> {
        let (name, arguments) = match kind {
            McpToolKind::Recall => (
                "recall",
                serde_json::json!({ "query": "sqlite concurrency", "limit": 10 }),
            ),
            McpToolKind::Remember => (
                "remember",
                serde_json::json!({ "content": format!("mixed MCP write {index}") }),
            ),
        };
        let id = self.next_request_id();
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": "tools/call",
            "params": { "name": name, "arguments": arguments },
        });
        let response = self.exchange(&request).await?;
        validate_mcp_tool_response(&response, id, kind)
    }

    async fn exchange(&mut self, request: &serde_json::Value) -> anyhow::Result<serde_json::Value> {
        self.write_frame(request).await?;
        let mut line = String::new();
        let bytes = self.reader.read_line(&mut line).await?;
        if bytes == 0 {
            bail!("MCP stdio closed before a response frame arrived");
        }
        serde_json::from_str(&line).context("deserialize MCP newline response")
    }

    async fn write_frame(&mut self, request: &serde_json::Value) -> anyhow::Result<()> {
        let mut frame = serde_json::to_vec(request)?;
        frame.push(b'\n');
        self.writer.write_all(&frame).await?;
        self.writer.flush().await?;
        Ok(())
    }

    async fn shutdown(&mut self) -> anyhow::Result<()> {
        self.writer.shutdown().await?;
        if let Some(server) = self.server.as_mut() {
            server.await.context("MCP stdio server task panicked")??;
        }
        self.server.take();
        Ok(())
    }

    async fn abort_and_reap(&mut self) {
        if let Some(server) = self.server.take() {
            server.abort();
            let _ = server.await;
        }
    }
}

impl Drop for McpStdioSession {
    fn drop(&mut self) {
        if let Some(server) = self.server.take() {
            server.abort();
        }
    }
}

fn validate_jsonrpc_result(response: &serde_json::Value, expected_id: u64) -> anyhow::Result<()> {
    if response.get("id") != Some(&serde_json::json!(expected_id)) {
        bail!("MCP response id did not match request {expected_id}");
    }
    if response.get("error").is_some_and(|error| !error.is_null()) {
        bail!("MCP response carried a top-level JSON-RPC error");
    }
    if !response
        .get("result")
        .is_some_and(serde_json::Value::is_object)
    {
        bail!("MCP response omitted its result object");
    }
    Ok(())
}

fn validate_mcp_tool_response(
    response: &serde_json::Value,
    expected_id: u64,
    kind: McpToolKind,
) -> anyhow::Result<Option<rb_types::MemoryId>> {
    validate_jsonrpc_result(response, expected_id)?;
    let result = response
        .get("result")
        .context("MCP response omitted its result")?;
    match result.get("isError") {
        Some(serde_json::Value::Bool(true)) => bail!("MCP tools/call returned isError=true"),
        Some(serde_json::Value::Bool(false)) | None => {}
        Some(_) => bail!("MCP tools/call returned a non-boolean isError"),
    }
    let structured = result
        .get("structuredContent")
        .context("MCP tools/call omitted structuredContent")?;
    match kind {
        McpToolKind::Remember => {
            let id = structured
                .get("id")
                .and_then(serde_json::Value::as_str)
                .context("MCP remember result omitted its id")?;
            let id = id
                .parse::<rb_types::MemoryId>()
                .context("MCP remember returned an invalid memory id")?;
            Ok(Some(id))
        }
        McpToolKind::Recall => {
            if !structured
                .get("results")
                .is_some_and(serde_json::Value::is_array)
            {
                bail!("MCP recall result omitted its results array");
            }
            Ok(None)
        }
    }
}

async fn run_mcp_operation(
    socket: &Path,
    kind: McpToolKind,
    index: usize,
    deadline: Duration,
) -> TimedOutcome<Option<rb_types::MemoryId>> {
    let timer = AbsoluteOperationDeadline::new(deadline);
    let mut session = match timer.run(McpStdioSession::start(socket)).await {
        Ok(Ok(session)) => session,
        Ok(Err(_)) => {
            return TimedOutcome::Error {
                elapsed: timer.elapsed(),
            };
        }
        Err(_) => {
            return TimedOutcome::Timeout {
                elapsed: timer.elapsed(),
            };
        }
    };
    let call = match timer.run(session.call_tool(kind, index)).await {
        Ok(result) => result,
        Err(_) => {
            session.abort_and_reap().await;
            return TimedOutcome::Timeout {
                elapsed: timer.elapsed(),
            };
        }
    };
    match timer.run(session.shutdown()).await {
        Ok(Ok(())) => match call {
            Ok(id) => TimedOutcome::Success {
                value: id,
                elapsed: timer.elapsed(),
            },
            Err(_) => TimedOutcome::Error {
                elapsed: timer.elapsed(),
            },
        },
        Ok(Err(_)) => TimedOutcome::Error {
            elapsed: timer.elapsed(),
        },
        Err(_) => {
            session.abort_and_reap().await;
            TimedOutcome::Timeout {
                elapsed: timer.elapsed(),
            }
        }
    }
}

async fn mixed_workload(
    socket: &Path,
    http_addr: std::net::SocketAddr,
    operations_per_path: usize,
    deadline: Duration,
) -> anyhow::Result<MixedOutcome> {
    let phase_started = Instant::now();
    let http_client = reqwest::Client::builder().build()?;
    let mut samples: std::collections::BTreeMap<&'static str, Samples> = MixedOp::ALL
        .iter()
        .map(|operation| {
            (
                operation.name(),
                Samples::new_at(operations_per_path, phase_started),
            )
        })
        .collect();
    let mut tasks = JoinSet::new();
    for operation in MixedOp::ALL {
        for index in 0..operations_per_path {
            let socket = socket.to_path_buf();
            let http_client = http_client.clone();
            tasks.spawn(async move {
                let outcome = match operation {
                    MixedOp::McpRecall => {
                        run_mcp_operation(&socket, McpToolKind::Recall, index, deadline).await
                    }
                    MixedOp::McpRemember => {
                        run_mcp_operation(&socket, McpToolKind::Remember, index, deadline).await
                    }
                    _ => {
                        timed_operation(
                            deadline,
                            run_mixed_operation(operation, &socket, http_addr, &http_client, index),
                        )
                        .await
                    }
                };
                (operation, outcome)
            });
        }
    }

    let mut acknowledged_ids = Vec::with_capacity(operations_per_path * 3);
    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok((operation, outcome)) => {
                if let Some(path_samples) = samples.get_mut(operation.name()) {
                    if let Some(Some(id)) = path_samples.record(outcome, true) {
                        acknowledged_ids.push(id);
                    }
                }
            }
            Err(error) => {
                for path_samples in samples.values_mut() {
                    path_samples.error(Duration::ZERO);
                }
                return Err(anyhow!("mixed workload task failed: {error}"));
            }
        }
    }

    let paths = MixedOp::ALL
        .iter()
        .map(|operation| {
            let path_samples = samples
                .remove(operation.name())
                .unwrap_or_else(|| Samples::new(0));
            NamedLatency {
                path: operation.name(),
                latency: path_samples.finish(),
            }
        })
        .collect();
    Ok(MixedOutcome {
        report: MixedWorkload {
            operations_per_path,
            paths,
            committed_writes: u64::try_from(acknowledged_ids.len()).unwrap_or(u64::MAX),
        },
        acknowledged_ids,
    })
}

async fn run_mixed_operation(
    operation: MixedOp,
    socket: &Path,
    http_addr: std::net::SocketAddr,
    http_client: &reqwest::Client,
    index: usize,
) -> anyhow::Result<Option<rb_types::MemoryId>> {
    match operation {
        MixedOp::UdsRecall => {
            let mut client = Client::connect(socket, Namespace::Project("scale".into())).await?;
            client
                .recall("sqlite concurrency".into(), None, vec![], 10)
                .await?;
            Ok(None)
        }
        MixedOp::UdsRemember => {
            let mut client = Client::connect(socket, Namespace::Project("scale".into())).await?;
            let id = client
                .remember(
                    format!("mixed UDS write {index}"),
                    None,
                    MemoryType::Insight,
                    5,
                    vec![],
                    vec![],
                    vec![],
                    Some(0.7),
                )
                .await?;
            Ok(Some(id))
        }
        MixedOp::HttpRecall => {
            let response = http_post(
                http_client,
                http_addr,
                "/recall",
                Request::Recall {
                    query: "sqlite concurrency".into(),
                    memory_type: None,
                    tags: vec![],
                    limit: 10,
                    filter: rb_types::RecallFilter::default(),
                },
            )
            .await?;
            if !response.status().is_success() {
                bail!("mixed HTTP recall returned {}", response.status());
            }
            Ok(None)
        }
        MixedOp::HttpRemember => {
            let response = http_post(
                http_client,
                http_addr,
                "/remember",
                Request::Remember {
                    content: format!("mixed HTTP write {index}"),
                    context: None,
                    memory_type: MemoryType::Insight,
                    importance: 5,
                    keywords: vec![],
                    tags: vec![],
                    related_files: vec![],
                    confidence: None,
                    supersedes: None,
                    anchors: vec![],
                },
            )
            .await?;
            if !response.status().is_success() {
                bail!("mixed HTTP remember returned {}", response.status());
            }
            let value = response
                .json::<serde_json::Value>()
                .await
                .context("decode mixed HTTP remember response")?;
            Ok(Some(parse_http_remember_id(&value)?))
        }
        MixedOp::McpRecall | MixedOp::McpRemember => {
            bail!("MCP mixed operations must use the stdio transport path")
        }
    }
}

fn parse_http_remember_id(value: &serde_json::Value) -> anyhow::Result<rb_types::MemoryId> {
    match serde_json::from_value::<Response>(value.clone())? {
        Response::Remembered { id } => Ok(id),
        other => bail!("HTTP remember returned unexpected response: {other:?}"),
    }
}

async fn http_post(
    client: &reqwest::Client,
    addr: std::net::SocketAddr,
    path: &str,
    request: Request,
) -> anyhow::Result<reqwest::Response> {
    Ok(client
        .post(format!("http://{addr}{path}"))
        .header("x-rusty-brain-namespace", "project:scale")
        .json(&shortcut_json(request)?)
        .send()
        .await?)
}

async fn http_recall(
    addr: std::net::SocketAddr,
    operations: usize,
    deadline: Duration,
) -> anyhow::Result<Latency> {
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
        let body = shortcut_json(request)?;
        let outcome = timed_operation(deadline, async {
            let response = client
                .post(&url)
                .header("x-rusty-brain-namespace", "project:scale")
                .json(&body)
                .send()
                .await?;
            if !response.status().is_success() {
                bail!("HTTP recall returned {}", response.status());
            }
            Ok(())
        })
        .await;
        samples.record(outcome, true);
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

fn read_pool_result(metrics: ReadPoolMetrics) -> ReadPoolResult {
    let acquired = metrics.acquired.max(1);
    ReadPoolResult {
        acquired: metrics.acquired,
        saturated: metrics.saturated,
        saturation_ratio: metrics.saturated as f64 / acquired as f64,
        average_wait_ms: metrics.total_wait_ns as f64 / acquired as f64 / 1_000_000.0,
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

fn git_worktree_is_clean() -> bool {
    std::process::Command::new("git")
        .args(["status", "--porcelain=v1", "--untracked-files=all"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .is_some_and(|output| output.stdout.is_empty())
}

async fn fault_evidence(
    shape: &EmbeddingShape,
    disk_dir: Option<&Path>,
    disk_max_mib: u64,
    interrupted_writes_artifact: Option<&Path>,
    writer_death_artifact: Option<&Path>,
    git_sha: &str,
) -> anyhow::Result<Vec<FaultEvidence>> {
    let provider_status = exercise_provider_timeout(shape).await?;
    let disk_evidence = disk_exhaustion_evidence(shape, disk_dir, disk_max_mib).await?;
    Ok(vec![
        FaultEvidence {
            case: "long_lived_reader",
            status: "executed_per_corpus",
            complete_for_full_eligibility: true,
            detail: "Reader transaction pins WAL during writes and graceful shutdown; report records WAL growth, blocked checkpoint duration, and explicit retry/truncate.".into(),
        },
        artifact_fault_evidence(
            "interrupted_writes",
            interrupted_writes_artifact,
            &["caught_writer_panic_isolates_and_does_not_lose_later_writes"],
            git_sha,
        ),
        provider_timeout_evidence(provider_status),
        artifact_fault_evidence(
            "writer_death_recovery",
            writer_death_artifact,
            &[
                "writer_alive_reports_liveness_and_flips_on_death",
                "writer_death_exits_the_accept_loop",
            ],
            git_sha,
        ),
        disk_evidence,
    ])
}

fn provider_timeout_evidence(provider_status: reqwest::StatusCode) -> FaultEvidence {
    let complete = provider_status == reqwest::StatusCode::SERVICE_UNAVAILABLE;
    FaultEvidence {
        case: "provider_timeout",
        status: if complete {
            "executed_expected_503"
        } else {
            "executed_unexpected_status"
        },
        complete_for_full_eligibility: complete,
        detail: format!(
            "A 200ms embedding provider behind a 20ms HTTP request deadline returned HTTP {provider_status}; full eligibility requires 503 Service Unavailable."
        ),
    }
}

fn open_pinned_reader(db: &Path) -> rusqlite::Result<rusqlite::Connection> {
    rusqlite::Connection::open_with_flags(db, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
}

fn artifact_fault_evidence(
    case: &'static str,
    artifact: Option<&Path>,
    required_tests: &[&str],
    expected_git_sha: &str,
) -> FaultEvidence {
    let tests = required_tests.join(", ");
    let Some(path) = artifact else {
        return FaultEvidence {
            case,
            status: "missing_immutable_artifact_reference",
            complete_for_full_eligibility: false,
            detail: format!(
                "Retain output for {tests} and pass its artifact path; a test name alone is not immutable evidence."
            ),
        };
    };
    match validate_and_hash_fault_artifact(path, expected_git_sha, required_tests) {
        Ok(digest) => FaultEvidence {
            case,
            status: "validated_hashed_artifact",
            complete_for_full_eligibility: true,
            detail: format!(
                "Retained test evidence for {tests}; artifact={}; sha256={digest}",
                path.display()
            ),
        },
        Err(error) => FaultEvidence {
            case,
            status: "unreadable_artifact_reference",
            complete_for_full_eligibility: false,
            detail: format!(
                "Could not validate retained test evidence for {tests} at {}: {error}",
                path.display()
            ),
        },
    }
}

fn validate_and_hash_fault_artifact(
    path: &Path,
    expected_git_sha: &str,
    required_tests: &[&str],
) -> anyhow::Result<String> {
    let raw = std::fs::read(path)
        .with_context(|| format!("read evidence artifact {}", path.display()))?;
    let record: FaultArtifactRecord = serde_json::from_slice(&raw)
        .with_context(|| format!("parse evidence artifact {}", path.display()))?;
    if record.schema != "rusty-brain-scale-fault-evidence-v1" {
        bail!("unsupported fault-evidence schema");
    }
    if record.git_sha != expected_git_sha || expected_git_sha == "unavailable" {
        bail!("fault evidence git SHA does not match the benchmark revision");
    }
    if record.exit_code != 0 {
        bail!("fault evidence command did not exit successfully");
    }
    if !record.command.contains("cargo test") {
        bail!("fault evidence command is not a cargo test invocation");
    }
    for test in required_tests {
        if !record.tests.iter().any(|recorded| recorded == test) || !record.output.contains(test) {
            bail!("fault evidence is missing required test {test}");
        }
    }
    if !record.output.contains("test result: ok") {
        bail!("fault evidence output does not record a passing test result");
    }
    Ok(format!("{:x}", Sha256::digest(&raw)))
}

fn mandatory_fault_evidence_complete(evidence: &[FaultEvidence]) -> bool {
    evidence.len() == 5
        && evidence
            .iter()
            .all(|item| item.complete_for_full_eligibility)
}

async fn disk_exhaustion_evidence(
    shape: &EmbeddingShape,
    disk_dir: Option<&Path>,
    max_mib: u64,
) -> anyhow::Result<FaultEvidence> {
    let Some(disk_dir) = disk_dir else {
        return Ok(FaultEvidence {
            case: "disk_full_low_disk",
            status: "not_run_requires_explicit_disposable_mount",
            complete_for_full_eligibility: false,
            detail: format!(
                "Pass --disk-exhaustion-dir only for a dedicated quota-limited mount root containing {DISPOSABLE_MARKER}; invalid/shared paths are refused."
            ),
        });
    };
    let detail = exercise_disk_exhaustion(shape, disk_dir, max_mib).await?;
    Ok(FaultEvidence {
        case: "disk_full_low_disk",
        status: "executed",
        complete_for_full_eligibility: true,
        detail,
    })
}

async fn exercise_disk_exhaustion(
    shape: &EmbeddingShape,
    disk_dir: &Path,
    max_mib: u64,
) -> anyhow::Result<String> {
    let root = disk_dir
        .canonicalize()
        .with_context(|| format!("canonicalize disk exhaustion dir {}", disk_dir.display()))?;
    if !root.join(DISPOSABLE_MARKER).is_file() {
        bail!(
            "refusing disk exhaustion: dedicated mount root {} lacks marker {}",
            root.display(),
            DISPOSABLE_MARKER
        );
    }
    let (available_before, mount_root) = filesystem_available(&root)?;
    if mount_root.canonicalize()? != root {
        bail!(
            "refusing disk exhaustion: {} is not the filesystem mount root ({})",
            root.display(),
            mount_root.display()
        );
    }
    let max_bytes = max_mib.saturating_mul(1024 * 1024);
    if available_before > max_bytes {
        bail!(
            "refusing disk exhaustion: disposable mount has {} MiB free, over --disk-exhaustion-max-mib {max_mib}",
            available_before / (1024 * 1024)
        );
    }

    let db = root.join("rusty-brain-scale-disk.db");
    let wal = root.join("rusty-brain-scale-disk.db-wal");
    let shm = root.join("rusty-brain-scale-disk.db-shm");
    let filler = root.join("rusty-brain-scale-filler.bin");
    for path in [&db, &wal, &shm, &filler] {
        if path.exists() {
            bail!(
                "refusing to overwrite pre-existing disk probe artifact {}",
                path.display()
            );
        }
    }
    let _cleanup = DiskProbeCleanup {
        db: db.clone(),
        filler: filler.clone(),
    };

    let namespace = Namespace::Project("disk-probe".into());
    let mut baseline = MemoryNote::new(
        namespace.clone(),
        "baseline committed before disk exhaustion".into(),
        MemoryType::Insight,
        5,
    );
    baseline.embedding_model = shape.model_id.clone();
    baseline.embedding_input_version = INPUT_VERSION.into();
    {
        let store = SqliteStore::open_with_model(&db, shape.dimension, &shape.model_id)?;
        store.insert_memory_batch_for_benchmark(&[(baseline, vec![0.25; shape.dimension])])?;
        store.checkpoint_truncate()?;
    }

    let mut fill = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&filler)?;
    let block = vec![0_u8; 1024 * 1024];
    loop {
        let (available, _) = filesystem_available(&root)?;
        if available <= 1024 * 1024 {
            break;
        }
        if let Err(error) = fill.write_all(&block) {
            if error.raw_os_error() == Some(libc::ENOSPC) {
                break;
            }
            return Err(error.into());
        }
    }
    let _ = fill.sync_all();
    drop(fill);

    let store = SqliteStore::open_with_model(&db, shape.dimension, &shape.model_id)?;
    let mut acknowledged = 0_u64;
    let mut refusal = None;
    for index in 0..64 {
        let mut note = MemoryNote::new(
            namespace.clone(),
            format!("disk pressure write {index} {}", "x".repeat(256 * 1024)),
            MemoryType::Insight,
            5,
        );
        note.embedding_model = shape.model_id.clone();
        note.embedding_input_version = INPUT_VERSION.into();
        match store.insert_memory_batch_for_benchmark(&[(note, vec![0.5; shape.dimension])]) {
            Ok(()) => acknowledged += 1,
            Err(error) => {
                refusal = Some(error.to_string());
                break;
            }
        }
    }
    drop(store);
    let refusal = refusal.context("disk probe never reached a write refusal")?;
    std::fs::remove_file(&filler)?;

    let reopened = SqliteStore::open_with_model(&db, shape.dimension, &shape.model_id)?;
    let stats =
        reopened.namespace_stats(&namespace, 30, &shape.model_id, INPUT_VERSION, 1, None)?;
    let expected = 1 + acknowledged;
    if stats.live != expected {
        bail!(
            "disk exhaustion lost acknowledged writes: expected {expected}, observed {}",
            stats.live
        );
    }
    drop(reopened);
    Ok(format!(
        "Dedicated mount started with {} MiB free; {acknowledged} pressure writes committed, the next failed with {refusal:?}, and all {expected} committed rows survived after freeing space and reopening.",
        available_before / (1024 * 1024)
    ))
}

fn filesystem_available(path: &Path) -> anyhow::Result<(u64, PathBuf)> {
    let output = std::process::Command::new("df")
        .args(["-Pk", path.to_string_lossy().as_ref()])
        .output()?;
    if !output.status.success() {
        bail!("df failed for {}", path.display());
    }
    let stdout = String::from_utf8(output.stdout)?;
    let line = stdout
        .lines()
        .last()
        .context("df returned no filesystem row")?;
    let fields: Vec<_> = line.split_whitespace().collect();
    if fields.len() < 6 {
        bail!("unexpected df output for {}: {line}", path.display());
    }
    let available_kib: u64 = fields[3].parse()?;
    Ok((available_kib.saturating_mul(1024), PathBuf::from(fields[5])))
}

struct DiskProbeCleanup {
    db: PathBuf,
    filler: PathBuf,
}

impl Drop for DiskProbeCleanup {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.filler);
        for suffix in ["", "-wal", "-shm"] {
            let path = PathBuf::from(format!("{}{suffix}", self.db.display()));
            let _ = std::fs::remove_file(path);
        }
    }
}

struct SlowProvider {
    dimension: usize,
}

#[async_trait]
impl EmbeddingProvider for SlowProvider {
    fn model_id(&self) -> &str {
        "scale-slow-provider"
    }

    fn dim(&self) -> usize {
        self.dimension
    }

    async fn embed(&self, texts: &[String], _kind: EmbedKind) -> rb_types::Result<Vec<Vec<f32>>> {
        tokio::time::sleep(Duration::from_millis(200)).await;
        Ok(texts.iter().map(|_| vec![0.25; self.dimension]).collect())
    }
}

async fn exercise_provider_timeout(shape: &EmbeddingShape) -> anyhow::Result<reqwest::StatusCode> {
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
    let daemon = Daemon::bind(
        config,
        SharedEmbedder::new(SlowProvider {
            dimension: shape.dimension,
        }),
    )
    .await?;
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
    Ok(status)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    struct FakeMcpProxy {
        remembered_id: rb_types::MemoryId,
    }

    #[async_trait]
    impl DaemonProxy for FakeMcpProxy {
        async fn call(&mut self, request: Request) -> rb_types::Result<Response> {
            Ok(match request {
                Request::Remember { .. } => Response::Remembered {
                    id: self.remembered_id.clone(),
                },
                Request::Recall { .. } => Response::Recalled {
                    results: Vec::new(),
                    degraded: false,
                },
                _ => Response::Pong {
                    contract_version: rb_proto::CONTRACT_VERSION,
                    recall_channels: None,
                },
            })
        }
    }

    fn latency_with(successes: usize, errors: usize, timeouts: usize) -> Latency {
        Latency {
            attempts: successes + errors,
            successes,
            errors,
            timeouts,
            residual_blocking_possible: timeouts > 0,
            error_p50_ms: 0.0,
            p50_ms: 0.0,
            p95_ms: 0.0,
            p99_ms: 0.0,
            throughput_per_sec: 0.0,
        }
    }

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

    #[test]
    fn defaults_use_the_real_384_dimensional_local_model() {
        let options = Options::parse_from(["scale_benchmark"]);
        assert!(matches!(options.provider, ProviderMode::Local));
        assert_eq!(options.dimension, 384);
        assert_eq!(options.model_id, "all-MiniLM-L6-v2");
        assert_eq!(options.operations, MINIMUM_P99_SAMPLES);
        assert_eq!(options.burst, MINIMUM_P99_SAMPLES);
    }

    #[test]
    fn p99_adequacy_rejects_99_actual_successes() {
        assert!(!latency_is_adequate(&latency_with(99, 0, 0)));
    }

    #[test]
    fn p99_adequacy_accepts_100_error_free_actual_successes() {
        assert!(latency_is_adequate(&latency_with(100, 0, 0)));
    }

    #[tokio::test]
    async fn timeout_is_counted_as_a_timed_error_sample() {
        let outcome = timed_operation(Duration::from_millis(1), async {
            tokio::time::sleep(Duration::from_millis(50)).await;
            Ok(())
        })
        .await;
        let mut samples = Samples::new(1);
        samples.record(outcome, true);
        let latency = samples.finish();
        assert_eq!(
            (
                latency.attempts,
                latency.successes,
                latency.errors,
                latency.timeouts,
                latency.residual_blocking_possible,
            ),
            (1, 0, 1, 1, true)
        );
    }

    #[test]
    fn mcp_is_error_result_is_rejected() {
        let response = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 7,
            "result": {
                "content": [{"type": "text", "text": "failed"}],
                "structuredContent": {"id": rb_types::MemoryId::new().to_string()},
                "isError": true
            }
        });
        assert!(validate_mcp_tool_response(&response, 7, McpToolKind::Remember).is_err());
    }

    #[tokio::test]
    async fn benchmark_mcp_path_round_trips_through_stdio_framing() {
        let remembered_id = rb_types::MemoryId::new();
        let expected_id = remembered_id.clone();
        let mut session = tokio::time::timeout(
            Duration::from_secs(1),
            McpStdioSession::start_with_proxy(FakeMcpProxy { remembered_id }),
        )
        .await
        .unwrap()
        .unwrap();
        let returned_id = session
            .call_tool(McpToolKind::Remember, 0)
            .await
            .unwrap()
            .unwrap();
        session.shutdown().await.unwrap();
        assert_eq!(returned_id, expected_id);
    }

    #[tokio::test]
    async fn absolute_deadline_is_shared_across_operation_steps() {
        let deadline = AbsoluteOperationDeadline::new(Duration::from_millis(80));
        deadline
            .run(tokio::time::sleep(Duration::from_millis(45)))
            .await
            .unwrap();
        assert!(deadline
            .run(tokio::time::sleep(Duration::from_millis(45)))
            .await
            .is_err());
    }

    #[test]
    fn http_remember_response_returns_acknowledged_id() {
        let expected_id = rb_types::MemoryId::new();
        let value = serde_json::to_value(Response::Remembered {
            id: expected_id.clone(),
        })
        .unwrap();
        assert_eq!(parse_http_remember_id(&value).unwrap(), expected_id);
    }

    #[test]
    fn full_eligibility_rejects_missing_disk_fault_evidence() {
        let mut evidence: Vec<_> = (0..5)
            .map(|_| FaultEvidence {
                case: "test",
                status: "executed",
                complete_for_full_eligibility: true,
                detail: String::new(),
            })
            .collect();
        evidence[4].complete_for_full_eligibility = false;
        assert!(!mandatory_fault_evidence_complete(&evidence));
    }

    #[test]
    fn unexpected_provider_status_is_reported_as_incomplete_evidence() {
        let evidence = provider_timeout_evidence(reqwest::StatusCode::OK);

        assert_eq!(
            (evidence.status, evidence.complete_for_full_eligibility),
            ("executed_unexpected_status", false)
        );
    }

    #[test]
    fn pinned_reader_rejects_writes() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("reader.db");
        drop(rusqlite::Connection::open(&db).unwrap());

        let reader = open_pinned_reader(&db).unwrap();

        assert!(reader
            .execute_batch("CREATE TABLE forbidden (id INTEGER)")
            .is_err());
    }

    #[test]
    fn fault_artifact_is_read_and_hashed_by_the_harness() {
        let dir = tempfile::tempdir().unwrap();
        let artifact = dir.path().join("writer-test.json");
        let test_name = "writer test";
        let git_sha = "0123456789abcdef0123456789abcdef01234567";
        let raw = serde_json::to_vec(&serde_json::json!({
            "schema": "rusty-brain-scale-fault-evidence-v1",
            "git_sha": git_sha,
            "command": "cargo test writer_test",
            "exit_code": 0,
            "tests": [test_name],
            "output": "test writer test ... ok\ntest result: ok",
        }))
        .unwrap();
        std::fs::write(&artifact, &raw).unwrap();

        let evidence = artifact_fault_evidence("writer", Some(&artifact), &[test_name], git_sha);

        assert!(evidence.complete_for_full_eligibility);
        assert_eq!(evidence.status, "validated_hashed_artifact");
        assert!(evidence
            .detail
            .contains(&format!("sha256={:x}", Sha256::digest(&raw))));

        let wrong_revision = artifact_fault_evidence(
            "writer",
            Some(&artifact),
            &[test_name],
            "ffffffffffffffffffffffffffffffffffffffff",
        );
        assert!(!wrong_revision.complete_for_full_eligibility);
    }

    #[test]
    fn fixture_provider_preserves_requested_shape_and_identity() {
        let embedder = build_embedder(ProviderMode::Fixture, 16, "shape-test").unwrap();
        assert_eq!(embedder.dim(), 16);
        assert_eq!(embedder.model_id(), "shape-test");
    }

    #[tokio::test]
    async fn disk_probe_refuses_an_unmarked_shared_directory() {
        let dir = tempfile::tempdir().unwrap();
        let shape = EmbeddingShape {
            dimension: 8,
            model_id: "disk-test".into(),
        };
        let error = exercise_disk_exhaustion(&shape, dir.path(), 512)
            .await
            .unwrap_err();
        assert!(error.to_string().contains("lacks marker"));
    }
}

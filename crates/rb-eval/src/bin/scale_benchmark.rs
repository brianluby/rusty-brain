//! Reproducible scale/concurrency benchmark for Vikunja #57.

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
use rb_mcp::{DaemonProxy, JsonRpcRequest};
use rb_proto::{Client, Request, Response};
use rb_store::SqliteStore;
use rb_types::{MemoryNote, MemoryType, Namespace};
use serde::Serialize;
use tokio::sync::oneshot;
use tokio::task::{JoinHandle, JoinSet};

const INPUT_VERSION: &str = "v2-composite";
const DEFAULT_DIMENSION: usize = 384;
const DEFAULT_MODEL_ID: &str = "all-MiniLM-L6-v2";
const PRODUCTION_CORPORA: [usize; 3] = [1_000, 10_000, 25_000];
const DISPOSABLE_MARKER: &str = ".rusty-brain-scale-disposable-volume";

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
    host: String,
    embedding_dimension: usize,
    embedding_model_id: String,
    embedding_fixture: &'static str,
    production_envelope_eligible: bool,
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
    observed_live: u64,
    observed_live_after_reopen: u64,
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
struct MixedWorkload {
    operations_per_path: usize,
    paths: Vec<NamedLatency>,
    committed_writes: u64,
}

struct MixedOutcome {
    report: MixedWorkload,
    committed_writes: u64,
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
    if options.dimension == 0 {
        bail!("--dimension must be positive");
    }
    let embedder = build_embedder(options.provider, options.dimension, &options.model_id)?;
    let shape = EmbeddingShape {
        dimension: embedder.dim(),
        model_id: embedder.model_id().to_string(),
    };
    let corpus_sizes = parse_corpora(&options.corpora)?;
    let production_corpus_matrix = PRODUCTION_CORPORA
        .iter()
        .all(|required| corpus_sizes.contains(required));
    let queue_probe = run_queue_probe(1_024, &shape).await?;
    let mut corpora = Vec::with_capacity(corpus_sizes.len());
    for corpus_size in corpus_sizes.iter().copied() {
        corpora.push(
            run_corpus(
                corpus_size,
                options.operations,
                options.burst,
                &shape,
                embedder.clone(),
            )
            .await?,
        );
    }
    let production_envelope_eligible = matches!(options.provider, ProviderMode::Local)
        && shape.dimension == DEFAULT_DIMENSION
        && shape.model_id == DEFAULT_MODEL_ID
        && production_corpus_matrix
        && queue_probe.latency.errors == 0
        && queue_probe.no_lost_committed_writes
        && corpora.iter().all(|corpus| {
            corpus.adequate_latency_samples
                && corpus.no_lost_committed_writes
                && corpus.namespace_isolation
                && corpus.retry_checkpoint_cleared_wal
        });

    let report = Report {
        schema: "rusty-brain-scale-v2",
        generated_at: chrono::Utc::now().to_rfc3339(),
        git_sha: command_output("git", &["rev-parse", "HEAD"]),
        host: command_output("uname", &["-a"]),
        embedding_dimension: shape.dimension,
        embedding_model_id: shape.model_id.clone(),
        embedding_fixture: options.provider.description(),
        production_envelope_eligible,
        release_build_required: true,
        operations: options.operations,
        burst: options.burst,
        queue_probe,
        corpora,
        fault_matrix: fault_evidence(
            &shape,
            options.disk_exhaustion_dir.as_deref(),
            options.disk_exhaustion_max_mib,
        )
        .await?,
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
) -> anyhow::Result<CorpusResult> {
    let dir = tempfile::tempdir().context("create benchmark directory")?;
    let db = dir.path().join("memory.db");
    let seed_started = Instant::now();
    seed_corpus(&db, size, shape, &embedder).await?;
    let seed_seconds = seed_started.elapsed().as_secs_f64();

    let daemon = RunningDaemon::start(dir.path(), &db, embedder).await?;
    let dropped_before = rb_daemon::dropped_broadcast_count();
    let uds_recall = uds_recall(&daemon.socket, operations).await?;
    let http_recall = http_recall(daemon.http_addr, operations).await?;
    let uds_remember_result = uds_remember(&daemon.socket, operations, "interactive").await?;
    let (hook_burst, hook_committed) = hook_burst(&daemon.socket, burst).await;
    let mixed = mixed_workload(&daemon.socket, daemon.http_addr, burst.clamp(2, 32)).await?;
    let committed_writes =
        uds_remember_result.successes as u64 + hook_committed + mixed.committed_writes;
    let stats = daemon
        .store
        .namespace_stats(
            Namespace::Project("scale".into()),
            30,
            shape.model_id.clone(),
            INPUT_VERSION.to_string(),
            1,
            None,
        )
        .await
        .context("read post-load stats")?;
    let namespace_isolation = verify_namespace_isolation(&daemon.socket).await?;
    let queue = queue_result(daemon.store.writer_queue_metrics());
    let read_pool = read_pool_result(daemon.store.read_pool_metrics());
    let dropped_broadcasts = rb_daemon::dropped_broadcast_count() - dropped_before;
    let rss_bytes = current_rss_bytes();

    let reader = rusqlite::Connection::open(&db).context("open long-lived reader")?;
    reader.execute_batch("BEGIN")?;
    let _: i64 = reader.query_row("SELECT COUNT(*) FROM memories", [], |row| row.get(0))?;
    let pinned_reader_writes = uds_remember(&daemon.socket, operations, "reader-pinned").await?;
    let wal_bytes_with_reader = file_size(&wal_path(&db));
    let shutdown_checkpoint = daemon.stop().await?;
    reader.execute_batch("COMMIT")?;
    drop(reader);

    let retry_started = Instant::now();
    let reopened = SqliteStore::open_with_model(&db, shape.dimension, &shape.model_id)?;
    reopened.checkpoint_truncate()?;
    let retry_checkpoint_ms = retry_started.elapsed().as_millis();
    let reopened_stats = reopened.namespace_stats(
        &Namespace::Project("scale".into()),
        30,
        &shape.model_id,
        INPUT_VERSION,
        1,
        None,
    )?;
    drop(reopened);

    let wal_bytes_after_retry = file_size(&wal_path(&db));
    let total_committed_writes = committed_writes + pinned_reader_writes.successes as u64;
    let expected_live = size as u64 + total_committed_writes;
    let adequate_latency_samples = operations >= 30
        && burst >= 30
        && mixed.report.operations_per_path >= 30
        && uds_recall.errors == 0
        && http_recall.errors == 0
        && uds_remember_result.errors == 0
        && hook_burst.errors == 0
        && pinned_reader_writes.errors == 0
        && mixed
            .report
            .paths
            .iter()
            .all(|path| path.latency.errors == 0);
    Ok(CorpusResult {
        corpus_size: size,
        seed_seconds,
        uds_recall,
        http_recall,
        uds_remember: uds_remember_result,
        hook_burst,
        pinned_reader_writes,
        mixed_workload: mixed.report,
        adequate_latency_samples,
        queue,
        read_pool,
        dropped_broadcasts,
        committed_writes: total_committed_writes,
        observed_live: stats.live,
        observed_live_after_reopen: reopened_stats.live,
        no_lost_committed_writes: reopened_stats.live == expected_live,
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

async fn run_queue_probe(operations: usize, shape: &EmbeddingShape) -> anyhow::Result<QueueProbe> {
    let dir = tempfile::tempdir()?;
    let db = dir.path().join("queue.db");
    let handle = StoreHandle::start_with_model(db, shape.dimension, shape.model_id.clone(), 4)?;
    let namespace = Namespace::Project("queue-probe".into());
    let started = Instant::now();
    let mut tasks = JoinSet::new();
    for index in 0..operations {
        let handle = handle.clone();
        let namespace = namespace.clone();
        let model_id = shape.model_id.clone();
        let dimension = shape.dimension;
        tasks.spawn(async move {
            let op_started = Instant::now();
            let mut note = MemoryNote::new(
                namespace,
                format!("queue saturation probe {index}"),
                MemoryType::Insight,
                5,
            );
            note.embedding_model = model_id;
            note.embedding_input_version = INPUT_VERSION.to_string();
            let result = handle.write(note, Some(vec![0.25; dimension])).await;
            (op_started.elapsed(), result)
        });
    }
    let mut durations = Vec::with_capacity(operations);
    let mut error_durations = Vec::new();
    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok((duration, Ok(()))) => durations.push(duration),
            Ok((duration, Err(_))) => error_durations.push(duration),
            Err(_) => error_durations.push(Duration::ZERO),
        }
    }
    let errors = error_durations.len();
    let metrics = handle.writer_queue_metrics();
    let stats = handle
        .namespace_stats(
            namespace,
            30,
            shape.model_id.clone(),
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
            error_durations,
            errors,
        }
        .finish(),
        queue: queue_result(metrics),
        observed_live,
        no_lost_committed_writes: observed_live == (operations - errors) as u64,
    })
}

async fn seed_corpus(
    db: &Path,
    size: usize,
    shape: &EmbeddingShape,
    embedder: &SharedEmbedder,
) -> anyhow::Result<()> {
    let store = SqliteStore::open_with_model(db, shape.dimension, &shape.model_id)?;
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
        let embeddings = embedder.embed(&inputs, EmbedKind::Document).await?;
        if embeddings.len() != notes.len()
            || embeddings
                .iter()
                .any(|embedding| embedding.len() != shape.dimension)
        {
            bail!("embedding provider returned the wrong batch shape");
        }
        let rows: Vec<_> = notes.into_iter().zip(embeddings).collect();
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
            let result = async {
                let mut client =
                    Client::connect(&socket, Namespace::Project("scale".into())).await?;
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
                Ok::<(), rb_types::Error>(())
            }
            .await;
            (op_started.elapsed(), result)
        });
    }
    let mut durations = Vec::with_capacity(burst);
    let mut error_durations = Vec::new();
    while let Some(result) = tasks.join_next().await {
        match result {
            Ok((duration, Ok(()))) => durations.push(duration),
            Ok((duration, Err(_))) => error_durations.push(duration),
            Err(_) => error_durations.push(Duration::ZERO),
        }
    }
    let errors = error_durations.len();
    let committed = durations.len() as u64;
    let samples = Samples {
        started,
        durations,
        error_durations,
        errors,
    };
    (samples.finish(), committed)
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

    fn is_write(self) -> bool {
        matches!(
            self,
            Self::UdsRemember | Self::HttpRemember | Self::McpRemember
        )
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

async fn mixed_workload(
    socket: &Path,
    http_addr: std::net::SocketAddr,
    operations_per_path: usize,
) -> anyhow::Result<MixedOutcome> {
    let started = Instant::now();
    let mut tasks = JoinSet::new();
    for operation in MixedOp::ALL {
        for index in 0..operations_per_path {
            let socket = socket.to_path_buf();
            tasks.spawn(async move {
                let op_started = Instant::now();
                let result = run_mixed_operation(operation, &socket, http_addr, index).await;
                (operation, op_started.elapsed(), result)
            });
        }
    }

    let mut samples: std::collections::BTreeMap<&'static str, (Vec<Duration>, Vec<Duration>)> =
        MixedOp::ALL
            .iter()
            .map(|operation| (operation.name(), (Vec::new(), Vec::new())))
            .collect();
    let mut committed_writes = 0_u64;
    while let Some(joined) = tasks.join_next().await {
        match joined {
            Ok((operation, elapsed, Ok(()))) => {
                if operation.is_write() {
                    committed_writes += 1;
                }
                if let Some((successes, _)) = samples.get_mut(operation.name()) {
                    successes.push(elapsed);
                }
            }
            Ok((operation, elapsed, Err(_))) => {
                if let Some((_, errors)) = samples.get_mut(operation.name()) {
                    errors.push(elapsed);
                }
            }
            Err(error) => return Err(anyhow!("mixed workload task failed: {error}")),
        }
    }

    let paths = MixedOp::ALL
        .iter()
        .map(|operation| {
            let (durations, error_durations) = samples
                .remove(operation.name())
                .unwrap_or_else(|| (Vec::new(), Vec::new()));
            let errors = error_durations.len();
            NamedLatency {
                path: operation.name(),
                latency: Samples {
                    started,
                    durations,
                    error_durations,
                    errors,
                }
                .finish(),
            }
        })
        .collect();
    Ok(MixedOutcome {
        report: MixedWorkload {
            operations_per_path,
            paths,
            committed_writes,
        },
        committed_writes,
    })
}

async fn run_mixed_operation(
    operation: MixedOp,
    socket: &Path,
    http_addr: std::net::SocketAddr,
    index: usize,
) -> anyhow::Result<()> {
    match operation {
        MixedOp::UdsRecall => {
            let mut client = Client::connect(socket, Namespace::Project("scale".into())).await?;
            client
                .recall("sqlite concurrency".into(), None, vec![], 10)
                .await?;
        }
        MixedOp::UdsRemember => {
            let mut client = Client::connect(socket, Namespace::Project("scale".into())).await?;
            client
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
        }
        MixedOp::HttpRecall => {
            let response = http_post(
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
        }
        MixedOp::HttpRemember => {
            let response = http_post(
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
        }
        MixedOp::McpRecall | MixedOp::McpRemember => {
            let client = Client::connect(socket, Namespace::Project("scale".into())).await?;
            let mut proxy = McpClientProxy { client };
            let (name, arguments) = if matches!(operation, MixedOp::McpRecall) {
                (
                    "recall",
                    serde_json::json!({ "query": "sqlite concurrency", "limit": 10 }),
                )
            } else {
                (
                    "remember",
                    serde_json::json!({ "content": format!("mixed MCP write {index}") }),
                )
            };
            let request: JsonRpcRequest = serde_json::from_value(serde_json::json!({
                "jsonrpc": "2.0",
                "id": index + 1,
                "method": "tools/call",
                "params": { "name": name, "arguments": arguments },
            }))?;
            let response = rb_mcp::handle_request(request, &mut proxy)
                .await
                .context("MCP tools/call returned no response")?;
            if let Some(error) = response.error {
                bail!("mixed MCP {name} failed: {}", error.message);
            }
        }
    }
    Ok(())
}

async fn http_post(
    addr: std::net::SocketAddr,
    path: &str,
    request: Request,
) -> anyhow::Result<reqwest::Response> {
    Ok(reqwest::Client::new()
        .post(format!("http://{addr}{path}"))
        .header("x-rusty-brain-namespace", "project:scale")
        .json(&shortcut_json(request)?)
        .send()
        .await?)
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

async fn fault_evidence(
    shape: &EmbeddingShape,
    disk_dir: Option<&Path>,
    disk_max_mib: u64,
) -> anyhow::Result<Vec<FaultEvidence>> {
    let provider_status = exercise_provider_timeout(shape).await?;
    let disk_evidence = disk_exhaustion_evidence(shape, disk_dir, disk_max_mib).await?;
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
        disk_evidence,
    ])
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
            detail: format!(
                "Pass --disk-exhaustion-dir only for a dedicated quota-limited mount root containing {DISPOSABLE_MARKER}; invalid/shared paths are refused."
            ),
        });
    };
    let detail = exercise_disk_exhaustion(shape, disk_dir, max_mib).await?;
    Ok(FaultEvidence {
        case: "disk_full_low_disk",
        status: "executed",
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

    #[test]
    fn defaults_use_the_real_384_dimensional_local_model() {
        let options = Options::parse_from(["scale_benchmark"]);
        assert!(matches!(options.provider, ProviderMode::Local));
        assert_eq!(options.dimension, 384);
        assert_eq!(options.model_id, "all-MiniLM-L6-v2");
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

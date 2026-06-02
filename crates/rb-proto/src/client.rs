//! Async client for the rusty-brain daemon over a Unix domain socket.

use crate::codec::bounded_framed;
use crate::frame::{read_frame, write_frame};
use crate::messages::{Handshake, HandshakeAck, Request, Response, CONTRACT_VERSION};
use crate::{response_error_to_error, Response as Resp};
use rb_types::{
    Error, MemoryChanged, MemoryId, MemoryNote, MemoryType, MemoryUpdates, Namespace, Result,
    SearchResult,
};
use std::path::Path;
use tokio::net::UnixStream;
use tokio_util::codec::{Framed, LengthDelimitedCodec};

/// A connected, handshaken client. Sends one `Request`, reads one `Response`.
#[derive(Debug)]
pub struct Client {
    framed: Framed<UnixStream, LengthDelimitedCodec>,
}

/// One item yielded by a live subscribe stream: either a change event or a
/// notice that the broadcast dropped `n` events for this slow subscriber.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubscribeItem {
    /// A committed change in the connection's namespace.
    Change(MemoryChanged),
    /// The subscriber fell behind; `n` events were dropped by the broadcast.
    Lagged(u64),
}

impl Client {
    /// Connect to the daemon socket, perform the versioned handshake, and verify
    /// the daemon speaks `CONTRACT_VERSION`. Fails closed on any version drift or
    /// a non-ok ack.
    pub async fn connect(socket_path: &Path, namespace: Namespace) -> Result<Client> {
        let stream = UnixStream::connect(socket_path)
            .await
            .map_err(|e| Error::from_io(&e))?;
        let mut framed = bounded_framed(stream);

        let handshake = Handshake {
            contract_version: CONTRACT_VERSION,
            namespace,
        };
        write_frame(&mut framed, &handshake).await?;

        let ack: HandshakeAck = read_frame(&mut framed).await?;
        if ack.contract_version != CONTRACT_VERSION {
            return Err(Error::Storage(format!(
                "contract version mismatch: client {CONTRACT_VERSION}, daemon {}",
                ack.contract_version
            )));
        }
        if !ack.ok {
            let detail = ack
                .message
                .unwrap_or_else(|| "handshake rejected".to_string());
            return Err(Error::Storage(format!("handshake rejected: {detail}")));
        }

        Ok(Client { framed })
    }

    /// Send one request and read one response.
    pub async fn request(&mut self, req: Request) -> Result<Response> {
        write_frame(&mut self.framed, &req).await?;
        let resp: Response = read_frame(&mut self.framed).await?;
        Ok(resp)
    }

    /// Open a live change-notification stream on this connection. After this
    /// returns `Ok`, the connection no longer follows request/response cadence;
    /// call [`recv_change`](Self::recv_change) in a loop to read streamed frames.
    /// The stream is scoped server-side to the handshake namespace.
    pub async fn subscribe(&mut self) -> Result<()> {
        write_frame(&mut self.framed, &Request::Subscribe).await
    }

    /// Read the next streamed item from a subscribe stream. Blocks until the
    /// daemon emits the next `Change`/`Lagged` frame; a closed stream surfaces as
    /// `Error::Io`. Any non-streamed response is a protocol violation.
    pub async fn recv_change(&mut self) -> Result<SubscribeItem> {
        let resp: Response = read_frame(&mut self.framed).await?;
        match resp {
            Response::Change(evt) => Ok(SubscribeItem::Change(evt)),
            Response::Lagged { dropped } => Ok(SubscribeItem::Lagged(dropped)),
            Resp::Error { kind, message } => Err(response_error_to_error(&kind, &message)),
            other => Err(Error::Storage(format!(
                "unexpected frame on subscribe stream: {other:?}"
            ))),
        }
    }
}

impl Client {
    /// Helper: turn an unexpected response (including a wire `Error`) into an
    /// `Err`. The daemon's `Response::Error` is mapped back to a domain error;
    /// any other unexpected variant is a protocol violation -> `Error::Storage`.
    fn unexpected(resp: Resp) -> Error {
        match resp {
            Resp::Error { kind, message } => response_error_to_error(&kind, &message),
            other => Error::Storage(format!("unexpected response: {other:?}")),
        }
    }

    /// Store a new memory; returns its id.
    #[allow(clippy::too_many_arguments)]
    pub async fn remember(
        &mut self,
        content: String,
        context: Option<String>,
        memory_type: MemoryType,
        importance: u8,
        keywords: Vec<String>,
        tags: Vec<String>,
        related_files: Vec<String>,
    ) -> Result<MemoryId> {
        let resp = self
            .request(Request::Remember {
                content,
                context,
                memory_type,
                importance,
                keywords,
                tags,
                related_files,
            })
            .await?;
        match resp {
            Resp::Remembered { id } => Ok(id),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Hybrid recall; returns ranked results.
    pub async fn recall(
        &mut self,
        query: String,
        memory_type: Option<MemoryType>,
        tags: Vec<String>,
        limit: usize,
    ) -> Result<Vec<SearchResult>> {
        let resp = self
            .request(Request::Recall {
                query,
                memory_type,
                tags,
                limit,
            })
            .await?;
        match resp {
            Resp::Recalled { results } => Ok(results),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Fetch a single memory by id.
    pub async fn get(&mut self, id: MemoryId) -> Result<Option<MemoryNote>> {
        let resp = self.request(Request::Get { id }).await?;
        match resp {
            Resp::Got { memory } => Ok(memory),
            other => Err(Self::unexpected(other)),
        }
    }

    /// List memories in scope.
    pub async fn list(
        &mut self,
        min_importance: Option<u8>,
        limit: usize,
    ) -> Result<Vec<MemoryNote>> {
        let resp = self
            .request(Request::List {
                min_importance,
                limit,
            })
            .await?;
        match resp {
            Resp::Listed { memories } => Ok(memories),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Walk the link graph from a memory.
    pub async fn graph(&mut self, id: MemoryId, depth: u8) -> Result<Vec<MemoryNote>> {
        let resp = self.request(Request::Graph { id, depth }).await?;
        match resp {
            Resp::GraphResult { memories } => Ok(memories),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Apply a partial update to a memory.
    pub async fn update(&mut self, id: MemoryId, updates: MemoryUpdates) -> Result<()> {
        let resp = self.request(Request::Update { id, updates }).await?;
        match resp {
            Resp::Updated => Ok(()),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Soft-archive a memory.
    pub async fn delete(&mut self, id: MemoryId) -> Result<()> {
        let resp = self.request(Request::Delete { id }).await?;
        match resp {
            Resp::Deleted => Ok(()),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Fetch the project context payload (recent, important, total).
    pub async fn context(&mut self) -> Result<(Vec<MemoryNote>, Vec<MemoryNote>, usize)> {
        let resp = self.request(Request::Context).await?;
        match resp {
            Resp::ContextResult {
                recent,
                important,
                total,
            } => Ok((recent, important, total)),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Round-trip ping; returns the daemon's contract version.
    pub async fn ping(&mut self) -> Result<u32> {
        let resp = self.request(Request::Ping).await?;
        match resp {
            Resp::Pong { contract_version } => Ok(contract_version),
            other => Err(Self::unexpected(other)),
        }
    }

    /// Trigger one bounded evolution-job pass on the daemon; returns the summary
    /// `(scanned, changed, skipped)`. The daemon performs all mutations through
    /// its single writer; this is just the wire trigger.
    pub async fn run_job(&mut self, job: rb_types::JobKind) -> Result<(u64, u64, u64)> {
        let resp = self.request(Request::RunJob { job }).await?;
        match resp {
            Resp::JobRan {
                scanned,
                changed,
                skipped,
            } => Ok((scanned, changed, skipped)),
            other => Err(Self::unexpected(other)),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use crate::{
        read_frame, write_frame, Handshake, HandshakeAck, Request, Response, CONTRACT_VERSION,
    };
    use rb_types::Namespace;
    use std::path::PathBuf;
    use tokio::net::{UnixListener, UnixStream};
    use tokio_util::codec::{Framed, LengthDelimitedCodec};

    fn socket_path() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.sock");
        (dir, path)
    }

    // Accept ONE connection, read the handshake, ack it (optionally with a
    // forced ack contract version to simulate drift), then echo one request as
    // a canned Pong response. Returns after serving a single connection.
    async fn run_responder(listener: UnixListener, ack_version: u32, ok: bool) {
        let (stream, _addr) = listener.accept().await.unwrap();
        let mut framed: Framed<UnixStream, LengthDelimitedCodec> =
            Framed::new(stream, LengthDelimitedCodec::new());

        let _hs: Handshake = read_frame(&mut framed).await.unwrap();
        let ack = HandshakeAck {
            contract_version: ack_version,
            ok,
            message: if ok {
                None
            } else {
                Some("version mismatch".into())
            },
        };
        write_frame(&mut framed, &ack).await.unwrap();
        if !ok {
            return;
        }

        // Serve exactly one request: reply Pong regardless of the request.
        let _req: Request = read_frame(&mut framed).await.unwrap();
        let resp = Response::Pong {
            contract_version: CONTRACT_VERSION,
        };
        write_frame(&mut framed, &resp).await.unwrap();
    }

    #[tokio::test]
    async fn connect_handshake_and_request_round_trip() {
        let (_dir, path) = socket_path();
        let listener = UnixListener::bind(&path).unwrap();
        let server = tokio::spawn(run_responder(listener, CONTRACT_VERSION, true));

        let mut client = Client::connect(&path, Namespace::Project("rusty-brain".into()))
            .await
            .unwrap();
        let resp = client.request(Request::Ping).await.unwrap();
        match resp {
            Response::Pong { contract_version } => {
                assert_eq!(contract_version, CONTRACT_VERSION);
            }
            other => panic!("expected Pong, got {other:?}"),
        }
        server.await.unwrap();
    }

    #[tokio::test]
    async fn connect_rejects_contract_version_mismatch() {
        let (_dir, path) = socket_path();
        let listener = UnixListener::bind(&path).unwrap();
        // Responder acks with a different contract version and ok=false.
        let server = tokio::spawn(run_responder(listener, CONTRACT_VERSION + 1, false));

        let err = Client::connect(&path, Namespace::Global).await.unwrap_err();
        assert!(
            matches!(err, rb_types::Error::Storage(_)),
            "version mismatch must fail connect, got {err:?}"
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn connect_to_missing_socket_is_io_error() {
        let (_dir, path) = socket_path();
        // Never bind a listener -> connect must fail with an IO error.
        let err = Client::connect(&path, Namespace::Global).await.unwrap_err();
        // Connect failure now carries the io::ErrorKind via Error::IoKind so the
        // client can classify auto-start policy without string matching.
        assert!(
            err.io_kind().is_some(),
            "missing socket should carry an io::ErrorKind, got {err:?}"
        );
    }
}

#[cfg(test)]
mod wrapper_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use crate::{
        error_to_response, read_frame, write_frame, Handshake, HandshakeAck, Request, Response,
        CONTRACT_VERSION,
    };
    use rb_types::{Error, MemoryId, MemoryNote, MemoryType, Namespace, SearchResult};
    use std::path::PathBuf;
    use tokio::net::{UnixListener, UnixStream};
    use tokio_util::codec::{Framed, LengthDelimitedCodec};

    fn socket_path() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wrap.sock");
        (dir, path)
    }

    fn note() -> MemoryNote {
        MemoryNote::new(
            Namespace::Global,
            "remembered body".into(),
            MemoryType::Insight,
            6,
        )
    }

    // Accept one connection, handshake, then answer each incoming Request with a
    // matching canned Response until the client disconnects.
    async fn serve(listener: UnixListener, fixed_id: MemoryId) {
        let (stream, _addr) = listener.accept().await.unwrap();
        let mut framed: Framed<UnixStream, LengthDelimitedCodec> =
            Framed::new(stream, LengthDelimitedCodec::new());

        let _hs: Handshake = read_frame(&mut framed).await.unwrap();
        write_frame(
            &mut framed,
            &HandshakeAck {
                contract_version: CONTRACT_VERSION,
                ok: true,
                message: None,
            },
        )
        .await
        .unwrap();

        loop {
            let req: Request = match read_frame(&mut framed).await {
                Ok(r) => r,
                Err(_) => break, // client closed
            };
            let resp = match req {
                Request::Remember { .. } => Response::Remembered {
                    id: fixed_id.clone(),
                },
                Request::Recall { .. } => Response::Recalled {
                    results: vec![SearchResult {
                        memory: note(),
                        score: 0.5,
                    }],
                },
                Request::Get { .. } => Response::Got {
                    memory: Some(note()),
                },
                Request::List { .. } => Response::Listed {
                    memories: vec![note()],
                },
                Request::Graph { .. } => Response::GraphResult {
                    memories: vec![note()],
                },
                Request::Update { .. } => Response::Updated,
                Request::Delete { .. } => Response::Deleted,
                Request::Context => Response::ContextResult {
                    recent: vec![note()],
                    important: vec![note()],
                    total: 2,
                },
                Request::Ping => Response::Pong {
                    contract_version: CONTRACT_VERSION,
                },
                Request::Subscribe => Response::Pong {
                    contract_version: CONTRACT_VERSION,
                },
                Request::RunJob { .. } => Response::JobRan {
                    scanned: 4,
                    changed: 2,
                    skipped: 2,
                },
            };
            write_frame(&mut framed, &resp).await.unwrap();
        }
    }

    async fn connect(path: &std::path::Path) -> Client {
        Client::connect(path, Namespace::Global).await.unwrap()
    }

    #[tokio::test]
    async fn typed_wrappers_return_domain_types() {
        let (_dir, path) = socket_path();
        let listener = UnixListener::bind(&path).unwrap();
        let fixed_id = MemoryId::new();
        let server = tokio::spawn(serve(listener, fixed_id.clone()));

        let mut c = connect(&path).await;

        let id = c
            .remember(
                "body".into(),
                Some("ctx".into()),
                MemoryType::Insight,
                6,
                vec!["k".into()],
                vec!["t".into()],
                vec![],
            )
            .await
            .unwrap();
        assert_eq!(id, fixed_id);

        let results = c.recall("q".into(), None, vec![], 5).await.unwrap();
        assert_eq!(results.len(), 1);
        assert!((results[0].score - 0.5).abs() < f32::EPSILON);

        let got = c.get(fixed_id.clone()).await.unwrap();
        assert!(got.is_some());

        let listed = c.list(Some(5), 10).await.unwrap();
        assert_eq!(listed.len(), 1);

        let graphed = c.graph(fixed_id.clone(), 2).await.unwrap();
        assert_eq!(graphed.len(), 1);

        c.update(fixed_id.clone(), Default::default())
            .await
            .unwrap();
        c.delete(fixed_id.clone()).await.unwrap();

        let (recent, important, total) = c.context().await.unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(important.len(), 1);
        assert_eq!(total, 2);

        let version = c.ping().await.unwrap();
        assert_eq!(version, CONTRACT_VERSION);

        let (scanned, changed, skipped) = c.run_job(rb_types::JobKind::LinkDecay).await.unwrap();
        assert_eq!((scanned, changed, skipped), (4, 2, 2));

        drop(c);
        server.await.unwrap();
    }

    // A responder that always returns a Response::Error, to prove wrappers map
    // wire errors back into rb_types::Error.
    async fn serve_error(listener: UnixListener) {
        let (stream, _addr) = listener.accept().await.unwrap();
        let mut framed: Framed<UnixStream, LengthDelimitedCodec> =
            Framed::new(stream, LengthDelimitedCodec::new());
        let _hs: Handshake = read_frame(&mut framed).await.unwrap();
        write_frame(
            &mut framed,
            &HandshakeAck {
                contract_version: CONTRACT_VERSION,
                ok: true,
                message: None,
            },
        )
        .await
        .unwrap();
        let _req: Request = read_frame(&mut framed).await.unwrap();
        let resp = error_to_response(&Error::NotFound(MemoryId::new()));
        write_frame(&mut framed, &resp).await.unwrap();
    }

    #[tokio::test]
    async fn wrapper_maps_wire_error_to_domain_error() {
        let (_dir, path) = socket_path();
        let listener = UnixListener::bind(&path).unwrap();
        let server = tokio::spawn(serve_error(listener));

        let mut c = connect(&path).await;
        let err = c.get(MemoryId::new()).await.unwrap_err();
        // NotFound degrades to Storage on the wire (see error.rs), but it is an
        // Err, not a falsely-successful None.
        assert!(matches!(err, Error::Storage(_)), "got {err:?}");
        server.await.unwrap();
    }
}

#[cfg(test)]
mod subscribe_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use crate::{
        read_frame, write_frame, Handshake, HandshakeAck, Request, Response, CONTRACT_VERSION,
    };
    use rb_types::{ChangeKind, MemoryChanged, MemoryId, Namespace};
    use std::path::PathBuf;
    use tokio::net::{UnixListener, UnixStream};
    use tokio_util::codec::{Framed, LengthDelimitedCodec};

    fn socket_path() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub.sock");
        (dir, path)
    }

    // Accept one connection, handshake, read exactly one Subscribe request, then
    // stream a Change frame followed by a Lagged frame, then close.
    async fn serve_stream(listener: UnixListener, change_id: MemoryId) {
        let (stream, _addr) = listener.accept().await.unwrap();
        let mut framed: Framed<UnixStream, LengthDelimitedCodec> =
            Framed::new(stream, LengthDelimitedCodec::new());
        let _hs: Handshake = read_frame(&mut framed).await.unwrap();
        write_frame(
            &mut framed,
            &HandshakeAck {
                contract_version: CONTRACT_VERSION,
                ok: true,
                message: None,
            },
        )
        .await
        .unwrap();

        let req: Request = read_frame(&mut framed).await.unwrap();
        assert!(matches!(req, Request::Subscribe), "expected Subscribe");

        write_frame(
            &mut framed,
            &Response::Change(MemoryChanged {
                id: change_id,
                namespace: Namespace::Global,
                kind: ChangeKind::Created,
            }),
        )
        .await
        .unwrap();
        write_frame(&mut framed, &Response::Lagged { dropped: 5 })
            .await
            .unwrap();
        // Drop closes the stream; the client's next recv_change returns Err(Io).
    }

    #[tokio::test]
    async fn subscribe_then_recv_change_and_lagged() {
        let (_dir, path) = socket_path();
        let listener = UnixListener::bind(&path).unwrap();
        let change_id = MemoryId::new();
        let server = tokio::spawn(serve_stream(listener, change_id.clone()));

        let mut client = Client::connect(&path, Namespace::Global).await.unwrap();
        client.subscribe().await.unwrap();

        match client.recv_change().await.unwrap() {
            SubscribeItem::Change(evt) => {
                assert_eq!(evt.id, change_id);
                assert_eq!(evt.kind, ChangeKind::Created);
            }
            other => panic!("expected Change, got {other:?}"),
        }
        match client.recv_change().await.unwrap() {
            SubscribeItem::Lagged(n) => assert_eq!(n, 5),
            other => panic!("expected Lagged, got {other:?}"),
        }
        // Stream closed -> the next recv is a transport error, not a hang.
        let err = client.recv_change().await.unwrap_err();
        assert!(matches!(err, Error::Io(_)), "closed stream -> Io: {err:?}");

        server.await.unwrap();
    }
}

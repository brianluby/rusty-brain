//! Async client for the rusty-brain daemon over a Unix domain socket.

use crate::frame::{read_frame, write_frame};
use crate::messages::{Handshake, HandshakeAck, Request, Response, CONTRACT_VERSION};
use rb_types::{Error, Namespace, Result};
use std::path::Path;
use tokio::net::UnixStream;
use tokio_util::codec::{Framed, LengthDelimitedCodec};

/// A connected, handshaken client. Sends one `Request`, reads one `Response`.
#[derive(Debug)]
pub struct Client {
    framed: Framed<UnixStream, LengthDelimitedCodec>,
}

impl Client {
    /// Connect to the daemon socket, perform the versioned handshake, and verify
    /// the daemon speaks `CONTRACT_VERSION`. Fails closed on any version drift or
    /// a non-ok ack.
    pub async fn connect(socket_path: &Path, namespace: Namespace) -> Result<Client> {
        let stream = UnixStream::connect(socket_path)
            .await
            .map_err(|e| Error::Io(format!("connect {}: {e}", socket_path.display())))?;
        let mut framed = Framed::new(stream, LengthDelimitedCodec::new());

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
        assert!(
            matches!(err, rb_types::Error::Io(_)),
            "missing socket should be Error::Io, got {err:?}"
        );
    }
}

//! W5a.3 transport genericization (HTTP PRD HTTP-3): `Client<S>` speaks the
//! same length-delimited JSON framing over ANY async stream, and the UDS path
//! is byte-for-byte unaffected (agreement test pins both transports).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use futures::SinkExt;
use rb_proto::{
    bounded_framed, read_frame, write_frame, Client, Handshake, HandshakeAck, Request, Response,
    CONTRACT_VERSION,
};
use rb_types::Namespace;
use tokio_stream::StreamExt;

/// Serve one scripted handshake + Ping exchange on the server end of any
/// stream, returning the RAW bytes of every frame the client sent.
async fn serve_one_ping<S>(stream: S) -> Vec<Vec<u8>>
where
    S: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
{
    let mut framed = bounded_framed(stream);
    let mut client_frames = Vec::new();

    // Handshake: capture raw bytes, decode, ack.
    let raw = framed.next().await.unwrap().unwrap();
    client_frames.push(raw.to_vec());
    let hs: Handshake = serde_json::from_slice(&raw).unwrap();
    assert_eq!(hs.contract_version, CONTRACT_VERSION);
    let ack = HandshakeAck {
        contract_version: CONTRACT_VERSION,
        ok: true,
        message: None,
        capabilities: vec![],
    };
    let bytes = serde_json::to_vec(&ack).unwrap();
    framed.send(bytes::Bytes::from(bytes)).await.unwrap();

    // One request: capture raw bytes, decode, answer Pong.
    let raw = framed.next().await.unwrap().unwrap();
    client_frames.push(raw.to_vec());
    let req: Request = serde_json::from_slice(&raw).unwrap();
    assert!(matches!(req, Request::Ping));
    write_frame(
        &mut framed,
        &Response::Pong {
            contract_version: CONTRACT_VERSION,
            recall_channels: None,
        },
    )
    .await
    .unwrap();
    client_frames
}

/// The genericized client handshakes and round-trips over an in-memory
/// duplex stream — a transport that is NOT a Unix socket.
#[tokio::test]
async fn client_handshakes_and_round_trips_over_a_generic_stream() {
    let (client_end, server_end) = tokio::io::duplex(64 * 1024);
    let server = tokio::spawn(serve_one_ping(server_end));

    let mut client = Client::handshake(client_end, Namespace::Global, None)
        .await
        .unwrap();
    let resp = client.request(Request::Ping).await.unwrap();
    assert!(matches!(resp, Response::Pong { .. }));

    server.await.unwrap();
}

/// AGREEMENT TEST: the same logical exchange over a real Unix socket and over
/// an in-memory duplex stream produces byte-identical frames — the UDS wire
/// contract is unaffected by the genericization.
#[tokio::test]
async fn uds_and_generic_transports_emit_identical_frames() {
    // Duplex leg.
    let (client_end, server_end) = tokio::io::duplex(64 * 1024);
    let duplex_server = tokio::spawn(serve_one_ping(server_end));
    let mut client = Client::handshake(client_end, Namespace::Global, None)
        .await
        .unwrap();
    client.request(Request::Ping).await.unwrap();
    let duplex_frames = duplex_server.await.unwrap();

    // UDS leg through the UNCHANGED public connect path.
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("sock");
    let listener = tokio::net::UnixListener::bind(&socket).unwrap();
    let uds_server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        serve_one_ping(stream).await
    });
    let mut client = Client::connect(&socket, Namespace::Global).await.unwrap();
    client.request(Request::Ping).await.unwrap();
    let uds_frames = uds_server.await.unwrap();

    assert_eq!(
        duplex_frames, uds_frames,
        "the two transports must emit byte-identical handshake and request frames"
    );
}

/// The generic handshake fails closed on a daemon that rejects (ok=false) —
/// same semantics as the UDS connect path.
#[tokio::test]
async fn generic_handshake_fails_closed_on_rejected_ack() {
    let (client_end, server_end) = tokio::io::duplex(64 * 1024);
    tokio::spawn(async move {
        let mut framed = bounded_framed(server_end);
        let _hs: Handshake = read_frame(&mut framed).await.unwrap();
        let ack = HandshakeAck {
            contract_version: CONTRACT_VERSION,
            ok: false,
            message: Some("nope".into()),
            capabilities: vec![],
        };
        write_frame(&mut framed, &ack).await.unwrap();
    });

    let err = Client::handshake(client_end, Namespace::Global, None)
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("handshake rejected"),
        "expected a rejected-handshake error, got {err}"
    );
}

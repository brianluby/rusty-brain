//! W5a.3 transport genericization (HTTP PRD HTTP-3): `Client<S>` speaks the
//! same length-delimited JSON framing over ANY async stream, and the UDS path
//! is byte-for-byte unaffected (agreement test pins both transports).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use futures::SinkExt;
use rb_proto::{
    bounded_framed, read_frame, write_frame, Client, Handshake, HandshakeAck, Request, Response,
    CONTRACT_VERSION,
};
use rb_types::{Error, MemoryId, MemoryUpdates, Namespace, ProjectContext};
use std::time::Duration;
use tokio::io::{AsyncReadExt, DuplexStream};
use tokio_stream::StreamExt;
use tokio_util::codec::{Framed, LengthDelimitedCodec};

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

async fn connected_pair() -> (
    Client<DuplexStream>,
    Framed<DuplexStream, LengthDelimitedCodec>,
) {
    let (client_end, server_end) = tokio::io::duplex(64 * 1024);
    let (client, peer) = tokio::join!(
        Client::handshake(client_end, Namespace::Global, None),
        async move {
            let mut peer = bounded_framed(server_end);
            let _: Handshake = read_frame(&mut peer).await.unwrap();
            write_frame(
                &mut peer,
                &HandshakeAck {
                    contract_version: CONTRACT_VERSION,
                    ok: true,
                    message: None,
                    capabilities: vec![],
                },
            )
            .await
            .unwrap();
            peer
        }
    );
    (client.unwrap(), peer)
}

async fn answer_ping(peer: &mut Framed<DuplexStream, LengthDelimitedCodec>) {
    assert!(matches!(read_frame(peer).await.unwrap(), Request::Ping));
    write_frame(
        peer,
        &Response::Pong {
            contract_version: CONTRACT_VERSION,
            recall_channels: None,
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn cancelled_context_cannot_send_recall_or_consume_the_delayed_response() {
    let (mut client, mut peer) = connected_pair().await;
    {
        let context = client.context();
        tokio::pin!(context);
        tokio::select! {
            result = &mut context => panic!("context completed before a response: {result:?}"),
            request = read_frame(&mut peer) => {
                assert!(matches!(request.unwrap(), Request::Context));
            }
        }
        // Start the timeout only after the peer has observed Context.
        assert!(tokio::time::timeout(Duration::from_millis(1), &mut context)
            .await
            .is_err());
    }
    write_frame(
        &mut peer,
        &Response::ContextResult {
            context: ProjectContext::default(),
        },
    )
    .await
    .unwrap();

    let recall = tokio::time::timeout(
        Duration::from_secs(1),
        client.recall("follow-up".into(), None, vec![], 5),
    )
    .await
    .expect("the broken connection must reject recall immediately");
    assert!(matches!(recall, Err(Error::Io(_))));
    assert!(matches!(client.recv_response().await, Err(Error::Io(_))));
    drop(client);
    assert!(read_frame::<_, Request>(&mut peer).await.is_err());
}

#[tokio::test]
async fn cancelled_partial_send_rejects_the_next_request() {
    let (mut client, mut peer) = connected_pair().await;
    let request = Request::Recall {
        query: "x".repeat(256 * 1024),
        memory_type: None,
        tags: vec![],
        limit: 5,
        filter: Default::default(),
    };
    {
        let send = client.send_request(&request);
        tokio::pin!(send);
        let mut prefix = [0; 4];
        tokio::select! {
            result = &mut send => panic!("send completed before the peer drained it: {result:?}"),
            result = peer.get_mut().read_exact(&mut prefix) => {
                result.unwrap();
                assert!(u32::from_be_bytes(prefix) > 64 * 1024);
            }
        }
        // Dropping the future here leaves an incomplete frame in the transport.
    }
    assert!(matches!(
        tokio::time::timeout(Duration::from_secs(1), client.ping())
            .await
            .expect("an interrupted send must reject new requests"),
        Err(Error::Io(_))
    ));
    assert!(matches!(client.recv_response().await, Err(Error::Io(_))));
}

#[tokio::test]
async fn split_requests_reject_overlap_but_complete_and_reuse_the_connection() {
    let (mut client, mut peer) = connected_pair().await;
    client.send_request(&Request::Ping).await.unwrap();
    assert!(matches!(
        client.request(Request::Context).await,
        Err(Error::Io(_))
    ));
    answer_ping(&mut peer).await;
    assert!(matches!(
        client.recv_response().await.unwrap(),
        Response::Pong { .. }
    ));

    let (ping, ()) = tokio::join!(client.ping(), answer_ping(&mut peer));
    assert_eq!(ping.unwrap(), CONTRACT_VERSION);
}

#[tokio::test]
async fn local_size_and_capability_rejections_send_nothing_and_keep_the_connection_healthy() {
    let (mut client, mut peer) = connected_pair().await;
    let oversized = Request::Recall {
        query: "x".repeat(rb_proto::MAX_FRAME_BYTES),
        memory_type: None,
        tags: vec![],
        limit: 5,
        filter: Default::default(),
    };
    assert!(matches!(
        client.send_request(&oversized).await,
        Err(Error::InvalidArgument(_))
    ));
    assert!(matches!(
        client
            .request(Request::Update {
                id: MemoryId::new(),
                updates: MemoryUpdates {
                    human_curated: Some(true),
                    ..Default::default()
                },
            })
            .await,
        Err(Error::InvalidArgument(_))
    ));
    let (ping, ()) = tokio::join!(client.ping(), answer_ping(&mut peer));
    assert_eq!(ping.unwrap(), CONTRACT_VERSION);
}

#[tokio::test]
async fn typed_server_errors_leave_completed_roundtrips_reusable() {
    let (mut client, mut peer) = connected_pair().await;
    let (context, ()) = tokio::join!(client.context(), async {
        assert!(matches!(
            read_frame(&mut peer).await.unwrap(),
            Request::Context
        ));
        write_frame(
            &mut peer,
            &Response::Error {
                kind: "invalid_argument".into(),
                message: "context rejected".into(),
            },
        )
        .await
        .unwrap();
    });
    assert!(matches!(context, Err(Error::InvalidArgument(_))));
    let (subscribe, ()) = tokio::join!(client.subscribe(), async {
        assert!(matches!(
            read_frame(&mut peer).await.unwrap(),
            Request::Subscribe { .. }
        ));
        write_frame(
            &mut peer,
            &Response::Error {
                kind: "invalid_argument".into(),
                message: "subscribe rejected".into(),
            },
        )
        .await
        .unwrap();
    });
    assert!(matches!(subscribe, Err(Error::InvalidArgument(_))));
    let (ping, ()) = tokio::join!(client.ping(), answer_ping(&mut peer));
    assert_eq!(ping.unwrap(), CONTRACT_VERSION);
}

#[tokio::test]
async fn invalid_response_frame_prevents_sending_another_request() {
    let (mut client, mut peer) = connected_pair().await;
    client.send_request(&Request::Ping).await.unwrap();
    assert!(matches!(
        read_frame(&mut peer).await.unwrap(),
        Request::Ping
    ));
    peer.send(bytes::Bytes::from_static(b"not json"))
        .await
        .unwrap();
    assert!(matches!(
        client.recv_response().await,
        Err(Error::Serialization(_))
    ));
    assert!(matches!(client.ping().await, Err(Error::Io(_))));
    drop(client);
    assert!(read_frame::<_, Request>(&mut peer).await.is_err());
}

#[tokio::test]
async fn wrong_typed_response_prevents_sending_another_request() {
    let (mut client, mut peer) = connected_pair().await;
    let (context, ()) = tokio::join!(client.context(), async {
        assert!(matches!(
            read_frame(&mut peer).await.unwrap(),
            Request::Context
        ));
        write_frame(
            &mut peer,
            &Response::Pong {
                contract_version: CONTRACT_VERSION,
                recall_channels: None,
            },
        )
        .await
        .unwrap();
    });
    assert!(matches!(context, Err(Error::Storage(_))));
    assert!(matches!(client.ping().await, Err(Error::Io(_))));
    drop(client);
    assert!(read_frame::<_, Request>(&mut peer).await.is_err());
}

#[tokio::test]
async fn split_subscribe_keeps_stream_reads_separate_from_roundtrips() {
    let (mut client, mut peer) = connected_pair().await;
    client
        .send_request(&Request::Subscribe { since: Some(42) })
        .await
        .unwrap();
    assert!(matches!(
        read_frame(&mut peer).await.unwrap(),
        Request::Subscribe { since: Some(42) }
    ));
    write_frame(&mut peer, &Response::SubscribeAck)
        .await
        .unwrap();
    assert!(matches!(
        client.recv_response().await.unwrap(),
        Response::SubscribeAck
    ));
    // A cancelled stream read can resume: there is no request to misassociate.
    assert!(
        tokio::time::timeout(Duration::from_millis(1), client.recv_change())
            .await
            .is_err()
    );
    write_frame(&mut peer, &Response::Lagged { dropped: 5 })
        .await
        .unwrap();
    assert!(matches!(client.ping().await, Err(Error::Io(_))));
    assert!(matches!(client.recv_response().await, Err(Error::Io(_))));
    assert_eq!(
        client.recv_change().await.unwrap(),
        rb_proto::SubscribeItem::Lagged(5)
    );
    write_frame(&mut peer, &Response::Lagged { dropped: 7 })
        .await
        .unwrap();
    assert_eq!(
        client.recv_change().await.unwrap(),
        rb_proto::SubscribeItem::Lagged(7)
    );
    drop(client);
    assert!(read_frame::<_, Request>(&mut peer).await.is_err());
}

#[tokio::test]
async fn cancelled_subscribe_ack_cannot_be_reused_as_a_roundtrip_response() {
    let (mut client, mut peer) = connected_pair().await;
    {
        let subscribe = client.subscribe();
        tokio::pin!(subscribe);
        tokio::select! {
            result = &mut subscribe => panic!("subscribe completed without an ack: {result:?}"),
            request = read_frame(&mut peer) => {
                assert!(matches!(request.unwrap(), Request::Subscribe { .. }));
            }
        }
    }
    write_frame(&mut peer, &Response::SubscribeAck)
        .await
        .unwrap();
    assert!(matches!(client.ping().await, Err(Error::Io(_))));
    assert!(matches!(client.recv_change().await, Err(Error::Io(_))));
    drop(client);
    assert!(read_frame::<_, Request>(&mut peer).await.is_err());
}

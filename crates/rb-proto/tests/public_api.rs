#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use rb_proto::{
    error_to_response, read_frame, response_error_to_error, write_frame, Client, Handshake,
    HandshakeAck, Request, Response, CONTRACT_VERSION,
};
use rb_types::{Error, MemoryId, Namespace};

#[test]
fn public_surface_is_reachable_and_stable() {
    // Constants and messages.
    assert_eq!(CONTRACT_VERSION, 1);
    let _hs = Handshake {
        contract_version: CONTRACT_VERSION,
        namespace: Namespace::Global,
    };
    let _ack = HandshakeAck {
        contract_version: CONTRACT_VERSION,
        ok: true,
        message: None,
    };
    let _req = Request::Ping;

    // Error mapping helpers, round-tripping through the wire form.
    let resp = error_to_response(&Error::NotFound(MemoryId::new()));
    match resp {
        Response::Error { kind, message } => {
            assert_eq!(kind, "not_found");
            let back = response_error_to_error(&kind, &message);
            assert!(matches!(back, Error::Storage(_)));
        }
        other => panic!("expected Response::Error, got {other:?}"),
    }
}

// Compile-only references proving the framing and Client symbols are public.
// (Never called: these are type-level guards on the public surface.)
#[allow(dead_code)]
fn _framing_symbols_exist() {
    // Reference each public symbol so an accidental removal breaks compilation.
    let _ = write_frame::<tokio::net::UnixStream, Request>;
    let _ = read_frame::<tokio::net::UnixStream, Response>;
    let _ = Client::connect;
}

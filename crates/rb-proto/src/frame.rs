//! Length-delimited JSON framing over an async transport.
//!
//! Each frame is a 4-byte big-endian length prefix (managed by
//! `LengthDelimitedCodec`) followed by the serde_json-encoded payload bytes.

use futures::SinkExt;
use rb_types::{Error, Result};
use serde::de::DeserializeOwned;
use serde::Serialize;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_stream::StreamExt;
use tokio_util::codec::{Framed, LengthDelimitedCodec};

/// Serialize `value` to JSON and send it as one length-delimited frame.
///
/// `LengthDelimitedCodec` implements `Encoder<bytes::Bytes>`, so the sink item
/// is `bytes::Bytes`; `SinkExt::send` both feeds and flushes the frame.
pub async fn write_frame<S, T>(
    framed: &mut Framed<S, LengthDelimitedCodec>,
    value: &T,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin,
    T: Serialize,
{
    let bytes = serde_json::to_vec(value).map_err(|e| Error::Serialization(e.to_string()))?;
    framed
        .send(bytes::Bytes::from(bytes))
        .await
        .map_err(|e| Error::Io(e.to_string()))?;
    Ok(())
}

/// Read one length-delimited frame and deserialize it as `T`.
///
/// A clean end-of-stream (peer closed without sending a frame) yields
/// `Stream::next == None`, surfaced as `Error::Io` so callers can distinguish
/// it from a decode failure. The decoded item is `bytes::BytesMut`, which
/// derefs to `[u8]` for `serde_json::from_slice`.
pub async fn read_frame<S, T>(framed: &mut Framed<S, LengthDelimitedCodec>) -> Result<T>
where
    S: AsyncRead + AsyncWrite + Unpin,
    T: DeserializeOwned,
{
    match framed.next().await {
        Some(Ok(bytes)) => {
            serde_json::from_slice(&bytes).map_err(|e| Error::Serialization(e.to_string()))
        }
        Some(Err(e)) => Err(Error::Io(e.to_string())),
        None => Err(Error::Io(
            "connection closed before a frame was received".to_string(),
        )),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use crate::{Handshake, Request, CONTRACT_VERSION};
    use rb_types::{MemoryType, Namespace};
    use tokio_util::codec::{Framed, LengthDelimitedCodec};

    // A pair of in-memory bidirectional streams standing in for the two ends of
    // a UnixStream. Each end is wrapped in the length-delimited codec.
    fn framed_pair() -> (
        Framed<tokio::io::DuplexStream, LengthDelimitedCodec>,
        Framed<tokio::io::DuplexStream, LengthDelimitedCodec>,
    ) {
        let (a, b) = tokio::io::duplex(64 * 1024);
        (
            Framed::new(a, LengthDelimitedCodec::new()),
            Framed::new(b, LengthDelimitedCodec::new()),
        )
    }

    #[tokio::test]
    async fn write_then_read_round_trips_a_value() {
        let (mut client, mut server) = framed_pair();

        let hs = Handshake {
            contract_version: CONTRACT_VERSION,
            namespace: Namespace::Project("rusty-brain".into()),
            identity: None,
        };
        write_frame(&mut client, &hs).await.unwrap();

        let got: Handshake = read_frame(&mut server).await.unwrap();
        assert_eq!(got.contract_version, CONTRACT_VERSION);
        assert_eq!(got.namespace, Namespace::Project("rusty-brain".into()));
    }

    #[tokio::test]
    async fn two_frames_are_independently_decoded() {
        let (mut client, mut server) = framed_pair();

        let r1 = Request::Ping;
        let r2 = Request::Recall {
            query: "transactions".into(),
            memory_type: Some(MemoryType::BugFix),
            tags: vec![],
            limit: 5,
        };
        write_frame(&mut client, &r1).await.unwrap();
        write_frame(&mut client, &r2).await.unwrap();

        let g1: Request = read_frame(&mut server).await.unwrap();
        let g2: Request = read_frame(&mut server).await.unwrap();
        assert_eq!(
            serde_json::to_string(&g1).unwrap(),
            serde_json::to_string(&r1).unwrap()
        );
        assert_eq!(
            serde_json::to_string(&g2).unwrap(),
            serde_json::to_string(&r2).unwrap()
        );
    }

    #[tokio::test]
    async fn read_after_peer_closes_is_io_error() {
        let (client, mut server) = framed_pair();
        // Drop the client end without sending anything -> stream ends cleanly.
        drop(client);
        let err = read_frame::<_, Request>(&mut server).await.unwrap_err();
        assert!(
            matches!(err, rb_types::Error::Io(_)),
            "clean EOF should surface as Error::Io, got {err:?}"
        );
    }
}

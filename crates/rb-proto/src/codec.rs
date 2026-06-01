//! Bounded length-delimited codec for the rusty-brain wire protocol.
//!
//! All frame construction must go through [`bounded_codec`] so that oversized
//! frames are rejected at the transport layer, not silently buffered into an
//! OOM or panic.

use tokio::io::{AsyncRead, AsyncWrite};
use tokio_util::codec::{Framed, LengthDelimitedCodec};

/// Maximum bytes allowed in a single wire frame (1 MiB).
pub const MAX_FRAME_BYTES: usize = 1024 * 1024; // 1 MiB

/// Build a `LengthDelimitedCodec` capped at [`MAX_FRAME_BYTES`].
///
/// Use this factory everywhere a `LengthDelimitedCodec` is constructed for
/// the daemon protocol. A frame whose length prefix exceeds `MAX_FRAME_BYTES`
/// is rejected with an `io::Error`, surfaced as `Error::Io` by the frame
/// helpers.
pub fn bounded_codec() -> LengthDelimitedCodec {
    LengthDelimitedCodec::builder()
        .max_frame_length(MAX_FRAME_BYTES)
        .new_codec()
}

/// Wrap `stream` in the bounded length-delimited codec.
pub fn bounded_framed<S>(stream: S) -> Framed<S, LengthDelimitedCodec>
where
    S: AsyncRead + AsyncWrite,
{
    Framed::new(stream, bounded_codec())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use crate::frame::read_frame;
    use crate::Request;
    use futures::SinkExt;
    use tokio::io::duplex;
    use tokio_stream::StreamExt;
    use tokio_util::codec::Framed;

    /// A frame whose length prefix exceeds MAX_FRAME_BYTES must be rejected
    /// as Error::Io, not buffered into an OOM or panicked.
    #[tokio::test]
    async fn oversized_frame_is_rejected_as_io_error() {
        // DuplexStream pipe: writer side uses an unbounded codec so we can
        // actually send the oversized frame; reader side uses the bounded
        // codec under test.
        let (writer_raw, reader_raw) = duplex(4 * 1024 * 1024);
        let mut writer: Framed<_, LengthDelimitedCodec> =
            Framed::new(writer_raw, LengthDelimitedCodec::new());
        let mut reader: Framed<_, LengthDelimitedCodec> = Framed::new(reader_raw, bounded_codec());

        // Build a payload just over the limit so the length-prefix exceeds
        // MAX_FRAME_BYTES. The content doesn't matter — the codec rejects
        // based on the length prefix before reading the body.
        let oversized_payload = bytes::Bytes::from(vec![b'x'; MAX_FRAME_BYTES + 1]);
        writer.send(oversized_payload).await.unwrap();

        // The bounded reader must reject this frame.
        let err = read_frame::<_, Request>(&mut reader).await.unwrap_err();
        assert!(
            matches!(err, rb_types::Error::Io(_)),
            "oversized frame must be Error::Io, got {err:?}"
        );
    }

    #[tokio::test]
    async fn frame_at_limit_is_accepted() {
        let (writer_raw, reader_raw) = duplex(2 * 1024 * 1024);
        let mut writer: Framed<_, LengthDelimitedCodec> =
            Framed::new(writer_raw, LengthDelimitedCodec::new());
        let mut reader: Framed<_, LengthDelimitedCodec> = Framed::new(reader_raw, bounded_codec());

        writer
            .send(bytes::Bytes::from(vec![b'x'; MAX_FRAME_BYTES]))
            .await
            .unwrap();

        let frame = reader.next().await.unwrap().unwrap();
        assert_eq!(frame.len(), MAX_FRAME_BYTES);
    }
}

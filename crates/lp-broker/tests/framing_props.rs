//! Property tests for the broker's frame reader and writer (phase 5.1). The reader is the first
//! thing an unauthenticated local process reaches, so it must never panic, never read past a
//! frame, and never hold more than the limit.

use std::future::Future;
use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::Duration;

use lp_broker::framing::{MAX_MESSAGE, read_message, read_message_timed, write_message};
use proptest::collection::vec;
use proptest::prelude::*;
use tokio::io::{AsyncRead, ReadBuf};

/// 128 cases by default; `PROPTEST_CASES` overrides it.
fn config() -> ProptestConfig {
    let cases = std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(128);
    ProptestConfig::with_cases(cases)
}

fn block_on<F: Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime")
        .block_on(future)
}

/// A reader that hands out one byte per read, like the slowest possible pipe.
struct OneByte<'a>(&'a [u8]);

impl AsyncRead for OneByte<'_> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if buf.remaining() > 0
            && let Some((first, rest)) = self.0.split_first()
        {
            buf.put_slice(&[*first]);
            self.0 = rest;
        }
        Poll::Ready(Ok(()))
    }
}

fn framed(payloads: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    block_on(async {
        for p in payloads {
            write_message(&mut out, p).await.expect("write");
        }
    });
    out
}

/// Everything `read_message` returns for `input`, ending at the first `None` or error.
fn read_all(mut input: &[u8]) -> (Vec<Vec<u8>>, Option<io::ErrorKind>) {
    block_on(async {
        let mut frames = Vec::new();
        loop {
            match read_message(&mut input).await {
                Ok(Some(frame)) => frames.push(frame),
                Ok(None) => return (frames, None),
                Err(e) => return (frames, Some(e.kind())),
            }
        }
    })
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn frame_roundtrip_any_payload(payload in vec(any::<u8>(), 0..=65536)) {
        let wire = framed(std::slice::from_ref(&payload));
        prop_assert_eq!(wire.len(), payload.len() + 4);
        let (frames, end) = read_all(&wire);
        prop_assert_eq!(frames, vec![payload]);
        prop_assert_eq!(end, None);
    }

    #[test]
    fn frame_stream_preserves_order_and_ends_clean(
        payloads in vec(vec(any::<u8>(), 0..2048), 0..10),
    ) {
        let wire = framed(&payloads);
        let (frames, end) = read_all(&wire);
        prop_assert_eq!(frames, payloads);
        prop_assert_eq!(end, None);
    }

    #[test]
    fn frame_reader_survives_one_byte_reads(
        payloads in vec(vec(any::<u8>(), 0..300), 0..6),
        cut in any::<prop::sample::Index>(),
    ) {
        let wire = framed(&payloads);
        let whole = read_all(&wire);
        // The same stream, and the same stream cut at a random place, read a byte at a time.
        for input in [&wire[..], &wire[..cut.index(wire.len() + 1)]] {
            let expected = read_all(input);
            let got = block_on(async {
                let mut reader = OneByte(input);
                let mut frames = Vec::new();
                loop {
                    match read_message(&mut reader).await {
                        Ok(Some(frame)) => frames.push(frame),
                        Ok(None) => return (frames, None),
                        Err(e) => return (frames, Some(e.kind())),
                    }
                }
            });
            prop_assert_eq!(got, expected);
        }
        prop_assert_eq!(whole.0, payloads);
    }

    #[test]
    fn frame_reader_never_panics_on_random_bytes(bytes in vec(any::<u8>(), 0..512)) {
        let (frames, _) = read_all(&bytes);
        // Every frame costs at least its 4 header bytes, so the loop always ends.
        prop_assert!(frames.len() <= bytes.len() / 4);
        for frame in &frames {
            prop_assert!(frame.len() <= MAX_MESSAGE);
        }
        let consumed: usize = frames.iter().map(|f| f.len() + 4).sum();
        prop_assert!(consumed <= bytes.len());
    }

    #[test]
    fn frame_oversize_header_consumes_nothing(
        len in (MAX_MESSAGE as u32 + 1)..=u32::MAX,
        tail in vec(any::<u8>(), 0..64),
    ) {
        let mut wire = len.to_le_bytes().to_vec();
        wire.extend_from_slice(&tail);
        let mut reader = &wire[..];
        let error = block_on(read_message(&mut reader)).expect_err("too large");
        prop_assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        // Only the four header bytes were taken: the "body" was never read.
        prop_assert_eq!(reader, &tail[..]);
    }

    #[test]
    fn frame_write_refuses_oversize_and_writes_nothing(extra in 1usize..4096, byte in any::<u8>()) {
        let body = vec![byte; MAX_MESSAGE + extra];
        let mut out = Vec::new();
        let error = block_on(write_message(&mut out, &body)).expect_err("too large");
        prop_assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        prop_assert!(out.is_empty());
    }

    /// S7, pinned: a header cut after 1 to 3 bytes is reported as a clean close, like a stream
    /// that ends between messages. A body cut anywhere is an error.
    #[test]
    fn frame_truncated_length_prefix_is_pinned(
        payload in vec(any::<u8>(), 1..200),
        header_cut in 1usize..4,
    ) {
        let wire = framed(std::slice::from_ref(&payload));
        let (frames, end) = read_all(&wire[..header_cut]);
        prop_assert!(frames.is_empty());
        prop_assert_eq!(end, None);

        let body_cut = 4 + (payload.len() - 1) / 2;
        let (frames, end) = read_all(&wire[..body_cut]);
        prop_assert!(frames.is_empty());
        prop_assert_eq!(end, Some(io::ErrorKind::UnexpectedEof));
    }

    #[test]
    fn timed_reader_agrees_with_the_plain_reader(bytes in vec(any::<u8>(), 0..300), valid in any::<bool>()) {
        let wire = if valid { framed(&[bytes.clone(), bytes.clone()]) } else { bytes };
        let plain = read_all(&wire);
        let timed = block_on(async {
            let mut input = &wire[..];
            let mut frames = Vec::new();
            loop {
                let step = read_message_timed(
                    &mut input,
                    Duration::from_secs(30),
                    Duration::from_secs(30),
                )
                .await;
                match step {
                    Ok(Some(frame)) => frames.push(frame),
                    Ok(None) => return (frames, None),
                    Err(e) => return (frames, Some(e.kind())),
                }
            }
        });
        prop_assert_eq!(timed, plain);
    }
}

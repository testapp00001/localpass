//! Messages are a 4-byte little-endian length followed by that many bytes of JSON, the same
//! framing Chrome's native messaging uses, so the helper can pass bytes through unchanged.

use std::io;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::time::timeout;

/// Largest message accepted or sent. Chrome itself limits host-to-browser messages to 1 MiB.
pub const MAX_MESSAGE: usize = 1 << 20;

/// Reads one message. `Ok(None)` means the peer closed the stream between messages.
pub async fn read_message<R: AsyncRead + Unpin>(reader: &mut R) -> io::Result<Option<Vec<u8>>> {
    let mut len = [0u8; 4];
    match reader.read_exact(&mut len).await {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    read_body(reader, len).await.map(Some)
}

/// Like [`read_message`], but gives up (`TimedOut`) after `idle` without a new message, and
/// after `rest` once one has started, so a peer that goes quiet cannot hold a connection open.
pub async fn read_message_timed<R: AsyncRead + Unpin>(
    reader: &mut R,
    idle: Duration,
    rest: Duration,
) -> io::Result<Option<Vec<u8>>> {
    let timed_out = |what: &str| io::Error::new(io::ErrorKind::TimedOut, what.to_string());
    let mut len = [0u8; 4];
    match timeout(idle, reader.read_exact(&mut len)).await {
        Err(_) => return Err(timed_out("no message arrived in time")),
        Ok(Ok(_)) => {}
        Ok(Err(e)) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Ok(Err(e)) => return Err(e),
    }
    match timeout(rest, read_body(reader, len)).await {
        Err(_) => Err(timed_out("a message stopped half way")),
        Ok(body) => body.map(Some),
    }
}

async fn read_body<R: AsyncRead + Unpin>(reader: &mut R, len: [u8; 4]) -> io::Result<Vec<u8>> {
    let len = u32::from_le_bytes(len) as usize;
    if len > MAX_MESSAGE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "message is too large",
        ));
    }
    // The buffer grows as bytes arrive: a peer that announces a big message and then sends
    // nothing does not get a megabyte reserved for it.
    let mut body = Vec::with_capacity(len.min(16 * 1024));
    (&mut *reader)
        .take(len as u64)
        .read_to_end(&mut body)
        .await?;
    if body.len() < len {
        return Err(io::ErrorKind::UnexpectedEof.into());
    }
    Ok(body)
}

pub async fn write_message<W: AsyncWrite + Unpin>(writer: &mut W, body: &[u8]) -> io::Result<()> {
    if body.len() > MAX_MESSAGE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "message is too large",
        ));
    }
    writer.write_all(&(body.len() as u32).to_le_bytes()).await?;
    writer.write_all(body).await?;
    writer.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn messages_round_trip_and_eof_is_clean() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        write_message(&mut a, b"hello").await.unwrap();
        write_message(&mut a, b"").await.unwrap();
        drop(a);
        assert_eq!(read_message(&mut b).await.unwrap().unwrap(), b"hello");
        assert_eq!(read_message(&mut b).await.unwrap().unwrap(), b"");
        assert!(read_message(&mut b).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn oversized_and_truncated_messages_are_errors() {
        let (mut a, mut b) = tokio::io::duplex(1024);
        a.write_all(&(MAX_MESSAGE as u32 + 1).to_le_bytes())
            .await
            .unwrap();
        assert!(read_message(&mut b).await.is_err());

        let (mut a, mut b) = tokio::io::duplex(1024);
        a.write_all(&10u32.to_le_bytes()).await.unwrap();
        a.write_all(b"abc").await.unwrap();
        drop(a);
        assert!(read_message(&mut b).await.is_err());
        assert!(
            write_message(&mut b, &vec![0u8; MAX_MESSAGE + 1])
                .await
                .is_err()
        );
    }

    #[tokio::test(start_paused = true)]
    async fn quiet_peers_time_out_and_whole_messages_do_not() {
        let idle = Duration::from_secs(60);
        let rest = Duration::from_secs(5);

        // Nothing arrives: the idle limit applies.
        let (_a, mut b) = tokio::io::duplex(1024);
        let e = read_message_timed(&mut b, idle, rest).await.unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::TimedOut);

        // A header and then silence: the (shorter) limit for the rest applies.
        let (mut a, mut b) = tokio::io::duplex(1024);
        a.write_all(&1000u32.to_le_bytes()).await.unwrap();
        a.write_all(b"abc").await.unwrap();
        let started = tokio::time::Instant::now();
        let e = read_message_timed(&mut b, idle, rest).await.unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::TimedOut);
        assert!(started.elapsed() < idle);

        // A message that arrives in time, and a clean close.
        let (mut a, mut b) = tokio::io::duplex(1024);
        write_message(&mut a, b"hi").await.unwrap();
        drop(a);
        assert_eq!(
            read_message_timed(&mut b, idle, rest)
                .await
                .unwrap()
                .unwrap(),
            b"hi"
        );
        assert!(
            read_message_timed(&mut b, idle, rest)
                .await
                .unwrap()
                .is_none()
        );
    }
}

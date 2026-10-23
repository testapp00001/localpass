//! 2-byte big-endian length-prefixed frames (Noise messages are ≤ 65535 bytes).

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const MAX_FRAME: usize = 65535;

pub async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, data: &[u8]) -> std::io::Result<()> {
    if data.len() > MAX_FRAME {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "frame too large",
        ));
    }
    let mut buf = Vec::with_capacity(2 + data.len());
    buf.extend_from_slice(&(data.len() as u16).to_be_bytes());
    buf.extend_from_slice(data);
    w.write_all(&buf).await?;
    w.flush().await
}

pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R, buf: &mut Vec<u8>) -> std::io::Result<()> {
    let mut len = [0u8; 2];
    r.read_exact(&mut len).await?;
    let len = u16::from_be_bytes(len) as usize;
    buf.resize(len, 0);
    r.read_exact(buf).await?;
    Ok(())
}

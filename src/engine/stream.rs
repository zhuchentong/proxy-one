//! 隧道数据面工具：携带已读取前缀的流包装，以及「读到 HTTP 头部为止」的读取器。

use std::pin::Pin;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;

/// 在 [`TcpStream`] 外再包一层"先返回 prefix 字节"的读视图。
///
/// 建立 CONNECT/SOCKS5 隧道时，上游的应答头可能与应答体落在同一个 TCP
/// 分段里；`read_head` 读出的多余字节必须原样回放给数据面，否则会丢字节。
#[derive(Debug)]
pub struct PrefixedStream {
    prefix: Vec<u8>,
    pos: usize,
    inner: TcpStream,
}

impl PrefixedStream {
    pub fn new(inner: TcpStream, prefix: Vec<u8>) -> Self {
        Self {
            prefix,
            pos: 0,
            inner,
        }
    }
}

impl AsyncRead for PrefixedStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        if this.pos < this.prefix.len() {
            let n = (this.prefix.len() - this.pos).min(buf.remaining());
            buf.put_slice(&this.prefix[this.pos..this.pos + n]);
            this.pos += n;
            return Poll::Ready(Ok(()));
        }
        Pin::new(&mut this.inner).poll_read(cx, buf)
    }
}

impl AsyncWrite for PrefixedStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

/// 读取直到 `\r\n\r\n` 为止的头部，返回 `(头部, 多读到的剩余字节)`。
pub async fn read_head<S: AsyncRead + Unpin>(
    s: &mut S,
    cap: usize,
) -> std::io::Result<(Vec<u8>, Vec<u8>)> {
    let mut acc: Vec<u8> = Vec::with_capacity(1024);
    let mut chunk = [0u8; 4096];
    loop {
        if let Some(pos) = find_crlf2(&acc) {
            let leftover = acc[pos + 4..].to_vec();
            acc.truncate(pos + 4);
            return Ok((acc, leftover));
        }
        if acc.len() > cap {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "头部过大",
            ));
        }
        let n = s.read(&mut chunk).await?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "连接在对端读取头部前关闭",
            ));
        }
        acc.extend_from_slice(&chunk[..n]);
    }
}

fn find_crlf2(b: &[u8]) -> Option<usize> {
    b.windows(4).position(|w| w == b"\r\n\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn read_head_splits_head_and_leftover() {
        let data: &[u8] = b"GET / HTTP/1.1\r\nHost: a\r\n\r\nBODY-BYTES";
        let (head, leftover) = read_head(&mut &data[..], 1024).await.unwrap();
        assert!(head.starts_with(b"GET / HTTP/1.1"));
        assert!(head.ends_with(b"\r\n\r\n"));
        assert_eq!(leftover, b"BODY-BYTES");
    }

    #[tokio::test]
    async fn read_head_without_terminator_errors() {
        let data: &[u8] = b"no terminator here";
        assert!(read_head(&mut &data[..], 1024).await.is_err());
    }

    #[tokio::test]
    async fn read_head_respects_cap() {
        let data: &[u8] = &[b'x'; 64];
        assert!(read_head(&mut &data[..], 16).await.is_err());
    }
}

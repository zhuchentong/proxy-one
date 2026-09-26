//! 隧道数据面工具：携带已读取前缀的流包装，以及「读到 HTTP 头部为止」的读取器。

use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;

/// 记账回调：收到分段字节数，累加进对应方向的累计计数。
pub type TrafficCallback = Arc<dyn Fn(u64) + Send + Sync>;

/// 实时流量记账的流包装：写入/读出的每个分段都回调给记账闭包。
///
/// 包在上游隧道流外层，使转发字节随传输实时入账——长连接的累计流量
/// 与速率窗口才不会等到连接结束才更新；提前断开时已转发的字节也不会丢。
pub struct CountingStream<S> {
    inner: S,
    /// 客户端 → 上游（写入隧道的字节数）
    on_write: TrafficCallback,
    /// 上游 → 客户端（从隧道读到的字节数）
    on_read: TrafficCallback,
}

impl<S> CountingStream<S> {
    pub fn new(inner: S, on_write: TrafficCallback, on_read: TrafficCallback) -> Self {
        Self {
            inner,
            on_write,
            on_read,
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for CountingStream<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let before = buf.filled().len();
        let r = Pin::new(&mut this.inner).poll_read(cx, buf);
        if let Poll::Ready(Ok(())) = &r {
            let n = (buf.filled().len() - before) as u64;
            if n > 0 {
                (this.on_read)(n);
            }
        }
        r
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for CountingStream<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        let r = Pin::new(&mut this.inner).poll_write(cx, buf);
        if let Poll::Ready(Ok(n)) = &r
            && *n > 0
        {
            (this.on_write)(*n as u64);
        }
        r
    }
    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }
    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

/// 数据面记账回调对：`(写=上行, 读=下行)`，实时累加进该上游的累计流量。
pub fn traffic_callbacks(
    state: Arc<super::state::StateStore>,
    idx: usize,
) -> (TrafficCallback, TrafficCallback) {
    let up_state = state.clone();
    let on_write = Arc::new(move |n: u64| up_state.record_traffic(idx, n, 0));
    let on_read = Arc::new(move |n: u64| state.record_traffic(idx, 0, n));
    (on_write, on_read)
}

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

    #[tokio::test]
    async fn counting_stream_accounts_each_direction() {
        use std::sync::atomic::{AtomicU64, Ordering};
        use tokio::io::AsyncWriteExt;

        let (client, mut server) = tokio::io::duplex(64);
        let w = Arc::new(AtomicU64::new(0));
        let r = Arc::new(AtomicU64::new(0));
        let mut counted = CountingStream::new(
            client,
            Arc::new({
                let w = w.clone();
                move |n: u64| {
                    w.fetch_add(n, Ordering::Relaxed);
                }
            }),
            Arc::new({
                let r = r.clone();
                move |n: u64| {
                    r.fetch_add(n, Ordering::Relaxed);
                }
            }),
        );

        counted.write_all(b"HELLO").await.unwrap();
        counted.flush().await.unwrap();
        server.write_all(b"WORLD").await.unwrap();
        server.flush().await.unwrap();
        let mut buf = [0u8; 5];
        counted.read_exact(&mut buf).await.unwrap();

        assert_eq!(&buf, b"WORLD");
        assert_eq!(w.load(Ordering::Relaxed), 5);
        assert_eq!(r.load(Ordering::Relaxed), 5);
    }
}

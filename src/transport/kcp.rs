use std::borrow::{ BorrowMut};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;
use std::task::Context as TaskContext;

use super::{AddrMaybeCached, SocketOpts, Transport};
use crate::config::{TransportConfig};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use tokio::io;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{ ToSocketAddrs};
use tokio::sync::Mutex;
use tokio_kcp::{KcpConfig, KcpStream as RealKcpStream,KcpListener};

#[derive(Debug)]
pub struct KcpTransport {
    config: KcpConfig,
}

pub struct KcpStream {
     stream: RealKcpStream
}

impl Drop for KcpStream {
    fn drop(&mut self) {
        drop(self.stream.borrow_mut())
    }
}

impl AsyncRead for KcpStream {
    fn poll_read(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx,buf)
    }
}

impl AsyncWrite for KcpStream {
    fn poll_write(mut self: Pin<&mut Self>, cx: &mut TaskContext<'_>, buf: &[u8]) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx,buf)
    }

    fn poll_flush(self: Pin<&mut Self>, _cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_flush(_cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, _cx: &mut TaskContext<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().stream).poll_shutdown(_cx)
    }
}

#[cfg(unix)]
impl std::os::unix::io::AsRawFd for KcpStream {
    fn as_raw_fd(&self) -> std::os::unix::prelude::RawFd {
        self.stream.as_raw_fd()
    }
}

#[cfg(windows)]
impl std::os::windows::io::AsRawSocket for KcpStream {
    fn as_raw_socket(&self) -> std::os::windows::prelude::RawSocket {
        self.stream.as_raw_socket()
    }
}

impl core::fmt::Debug for KcpStream {
    fn fmt(&self, f: &mut core::fmt::Formatter) -> core::fmt::Result {
        write!(f, "Debug: KcpStream")
    }
}

#[async_trait]
impl Transport for KcpTransport {
    type Acceptor = Arc<Mutex<KcpListener>>;
    type RawStream = KcpStream;
    type Stream = KcpStream;

    fn new(config: &TransportConfig) -> Result<Self> {
        let config = config
            .kcp
            .as_ref()
            .ok_or_else(|| anyhow!("Missing kcp config"))?;
        Ok(KcpTransport {
            config: config.clone().into(),
        })
    }

    fn hint(_: &Self::Stream, _: SocketOpts) {

    }

    async fn bind<A: ToSocketAddrs + Send + Sync>(&self, addr: A) -> Result<Self::Acceptor> {
        let listener = KcpListener::bind(self.config,addr)
            .await
            .with_context(|| "Failed to create kcp listener")?;
        Ok(Arc::new(Mutex::new(listener)))
    }

    async fn accept(&self, a: &Self::Acceptor) -> Result<(Self::RawStream, SocketAddr)> {
        let incomming_result  = a.lock().await.accept().await.with_context(|| "Failed to accept KCP connection");
        match incomming_result {
            Ok((stream,addr)) => {
                Result::Ok((KcpStream{ stream }, addr))
            }
            Err(err) => {
                Result::Err(err)
            }
        }
    }

    async fn handshake(&self, conn: Self::RawStream) -> Result<Self::Stream> {
        Ok(conn)
    }

    async fn connect(&self, addr: &AddrMaybeCached) -> Result<Self::Stream> {
        let dest = addr.socket_addr.expect("bad kcp server address");
        let conn_result = RealKcpStream::connect(&self.config, dest).await;
        match conn_result{
            Ok(conn) => {
                Ok(KcpStream{ stream:conn})
            }
            Err(err) => {
                Err(err.into())
            }
        }
    }
}

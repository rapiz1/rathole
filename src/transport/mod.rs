use crate::config::TransportConfig;
use anyhow::Result;
use async_trait::async_trait;
use std::fmt::Debug;
use std::io;
use std::io::Error;
use std::net::SocketAddr;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::net::ToSocketAddrs;

// Specify a transport layer, like TCP, TLS
#[async_trait]
pub trait Transport: Debug + Send + Sync {
    type Acceptor: Send + Sync;
    type RawStream: Send + Sync;
    type ReliableStream: 'static + AsyncRead + AsyncWrite + Unpin + Send + Sync + Debug;
    type UnreliableStream: 'static + AsyncRead + AsyncWrite + Unpin + Send + Sync + Debug;


    async fn new(config: &TransportConfig) -> Result<Self>
    where
        Self: Sized;
    async fn bind<T: ToSocketAddrs + Send + Sync>(&self, addr: T) -> Result<Self::Acceptor>;
    async fn accept(&self, a: &Self::Acceptor) -> Result<(Self::RawStream, SocketAddr)>;

    /// Perform handshake using a newly initiated raw stream (tcp/udp)
    /// return a properly configured connection for protocol that transport uses.
    ///
    /// The returned connection may either be Reliable or Partially Reliable
    /// (wholly unreliable transport are not currently supported).
    ///
    /// Both Partially reliable and strictly reliable transport must provide a reliable stream
    /// If partially reliable, then an unreliable stream must additionally be provided
    async fn handshake(&self, conn: Self::RawStream) -> Result<TransportStream<Self>>;

    /// Connection to Server
    /// return
    ///   - A reliable ordered stream used for control channel communication
    ///   - Optionally an unordered and unreliable stream used for data channel forwarding
    ///     If no such stream is provided, then data will be sent using the reliable stream.
    async fn connect(&self, addr: &str) -> Result<TransportStream<Self>>;
}

#[derive(Debug)]
pub enum TransportStream<T: Transport + ?Sized> {
    StrictlyReliable(T::ReliableStream),
    PartiallyReliable(T::ReliableStream, T::UnreliableStream),
}

mod tcp;
pub use tcp::TcpTransport;
#[cfg(feature = "tls")]
mod tls;
#[cfg(feature = "tls")]
pub use tls::TlsTransport;

#[cfg(feature = "noise")]
mod noise;
#[cfg(feature = "noise")]
pub use noise::NoiseTransport;

impl<T> TransportStream<T>
    where T: Transport
{
    pub async fn write_all_reliably<'a>(&'a mut self, src: &'a [u8]) -> std::io::Result<()> {
        let r = match self {
            TransportStream::StrictlyReliable(s) => s.write_all(src).await,
            TransportStream::PartiallyReliable(s, _) => s.write_all(src).await,
        };
        r
    }

    pub(crate) fn get_reliable_stream(&mut self) -> &mut T::ReliableStream
    {
        match self {
            TransportStream::StrictlyReliable(s) => s,
            TransportStream::PartiallyReliable(s, _) => s,
        }
    }

    pub fn into_reliable_stream(self) -> T::ReliableStream {
        match self {
            TransportStream::StrictlyReliable(s) => s,
            TransportStream::PartiallyReliable(s, _) => s,
        }
    }
}

/// A dummy struct for use with transports that are strictly reliable
#[derive(Debug)]
pub struct UnimplementedUnreliableStream;

impl AsyncRead for UnimplementedUnreliableStream {
    fn poll_read(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &mut ReadBuf<'_>) -> Poll<io::Result<()>> {
        todo!()
    }
}

impl AsyncWrite for UnimplementedUnreliableStream {
    fn poll_write(self: Pin<&mut Self>, cx: &mut Context<'_>, buf: &[u8]) -> Poll<std::result::Result<usize, Error>> {
        todo!()
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::result::Result<(), Error>> {
        todo!()
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::result::Result<(), Error>> {
        todo!()
    }
}
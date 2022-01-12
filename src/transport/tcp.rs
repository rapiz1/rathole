use crate::config::TransportConfig;
use crate::helper::set_tcp_keepalive;

use super::Transport;
use anyhow::Result;
use async_trait::async_trait;
use std::net::SocketAddr;
use tokio::net::{TcpListener, TcpStream, ToSocketAddrs};
use crate::transport::{TransportStream, UnimplementedUnreliableStream};
use crate::transport::TransportStream::StrictlyReliable;

#[derive(Debug)]
pub struct TcpTransport {}

#[async_trait]
impl Transport for TcpTransport {
    type Acceptor = TcpListener;
    type ReliableStream = TcpStream;
    type UnreliableStream = UnimplementedUnreliableStream;
    type RawStream = TcpStream;

    async fn new(_config: &TransportConfig) -> Result<Self> {
        Ok(TcpTransport {})
    }

    async fn bind<T: ToSocketAddrs + Send + Sync>(&self, addr: T) -> Result<Self::Acceptor> {
        Ok(TcpListener::bind(addr).await?)
    }

    async fn accept(&self, a: &Self::Acceptor) -> Result<(Self::RawStream, SocketAddr)> {
        let (s, addr) = a.accept().await?;
        set_tcp_keepalive(&s);
        Ok((s, addr))
    }

    async fn handshake(&self, conn: Self::RawStream) -> Result<TransportStream<Self>> {
        Ok(StrictlyReliable(conn))
    }

    async fn connect(&self, addr: &str) -> Result<TransportStream<Self>> {
        let s = TcpStream::connect(addr).await?;
        set_tcp_keepalive(&s);
        Ok(StrictlyReliable(s)) // TCP cannot provide unreliable stream
    }
}

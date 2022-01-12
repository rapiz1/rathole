use crate::config::TransportConfig;
use crate::helper::set_tcp_keepalive;

use super::Transport;
use anyhow::Result;
use async_trait::async_trait;
use std::net::SocketAddr;
use tokio::net::{TcpListener, TcpStream, ToSocketAddrs};

#[derive(Debug)]
pub struct TcpTransport {}

#[async_trait]
impl Transport for TcpTransport {
    type Acceptor = TcpListener;
    type Stream = TcpStream;
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

    async fn handshake(&self, conn: Self::RawStream) -> Result<Self::Stream> {
        Ok(conn)
    }

    async fn connect(&self, addr: &str) -> Result<Self::Stream> {
        let s = TcpStream::connect(addr).await?;
        set_tcp_keepalive(&s);
        Ok(s)
    }
}

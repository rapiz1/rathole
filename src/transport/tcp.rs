use crate::config::TransportConfig;
use crate::helper::set_tcp_keepalive;

use super::Transport;
use anyhow::Result;
use async_trait::async_trait;
use std::net::SocketAddr;
use tokio::net::{TcpListener, TcpStream, ToSocketAddrs};
use tracing::error;

#[derive(Debug)]
pub struct TcpTransport {}

#[async_trait]
impl Transport for TcpTransport {
    type Acceptor = TcpListener;
    type Stream = TcpStream;

    async fn new(_config: &TransportConfig) -> Result<Box<Self>> {
        Ok(Box::new(TcpTransport {}))
    }

    async fn bind<T: ToSocketAddrs + Send + Sync>(&self, addr: T) -> Result<Self::Acceptor> {
        Ok(TcpListener::bind(addr).await?)
    }

    async fn accept(&self, a: &Self::Acceptor) -> Result<(Self::Stream, SocketAddr)> {
        let (s, addr) = a.accept().await?;
        Ok((s, addr))
    }

    async fn connect(&self, addr: &str) -> Result<Self::Stream> {
        let s = TcpStream::connect(addr).await?;
        if let Err(e) = set_tcp_keepalive(&s) {
            error!(
                "Failed to set TCP keepalive. The connection maybe unstable: {:?}",
                e
            );
        }
        Ok(s)
    }
}

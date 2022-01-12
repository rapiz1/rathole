use crate::config::TransportConfig;
use crate::helper::try_set_tcp_keepalive;
use anyhow::{Context, Result};
use async_trait::async_trait;
use std::fmt::Debug;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpStream, ToSocketAddrs};
use tracing::error;

static TCP_KEEPALIVE_SECS: u64 = 30;

/// Specify a transport layer, like TCP, TLS
#[async_trait]
pub trait Transport: Debug + Send + Sync {
    type Acceptor: Send + Sync;
    type RawStream: Send + Sync;
    type Stream: 'static + AsyncRead + AsyncWrite + Unpin + Send + Sync + Debug;

    fn new(config: &TransportConfig) -> Result<Self>
    where
        Self: Sized;
    /// Provide the transport with socket options, which can be handled at the need of the transport
    fn hint(conn: &Self::Stream, opts: SocketOpts);
    async fn bind<T: ToSocketAddrs + Send + Sync>(&self, addr: T) -> Result<Self::Acceptor>;
    async fn accept(&self, a: &Self::Acceptor) -> Result<(Self::RawStream, SocketAddr)>;
    async fn handshake(&self, conn: Self::RawStream) -> Result<Self::Stream>;
    async fn connect(&self, addr: &str) -> Result<Self::Stream>;
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

#[derive(Debug)]
pub struct SocketOpts {
    pub nodelay: Option<bool>,
    pub keepalive_secs: Option<u64>,
}

impl Default for SocketOpts {
    fn default() -> SocketOpts {
        SocketOpts {
            nodelay: Some(false),
            keepalive_secs: Some(TCP_KEEPALIVE_SECS),
        }
    }
}

impl SocketOpts {
    pub fn from(config: &TransportConfig) -> SocketOpts {
        SocketOpts {
            nodelay: Some(config.nodelay),
            ..Default::default()
        }
    }

    pub fn apply(&self, conn: &TcpStream) {
        if let Some(keepalive_secs) = self.keepalive_secs {
            if let Err(e) = try_set_tcp_keepalive(conn, Duration::from_secs(keepalive_secs))
                .with_context(|| "Failed to set keepalive")
            {
                error!("{:?}", e);
            }
        }

        if let Some(nodelay) = self.nodelay {
            if let Err(e) = conn
                .set_nodelay(nodelay)
                .with_context(|| "Failed to set nodelay")
            {
                error!("{:?}", e);
            }
        }
    }
}

use crate::config::{ClientServiceConfig, ServerServiceConfig, TransportConfig};
use crate::helper::try_set_tcp_keepalive;
use anyhow::{Context, Result};
use async_trait::async_trait;
use std::fmt::Debug;
use std::net::SocketAddr;
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::{TcpStream, ToSocketAddrs};
use tracing::{error, trace};

pub static DEFAULT_NODELAY: bool = false;

pub static DEFAULT_KEEPALIVE_SECS: u64 = 10;
pub static DEFAULT_KEEPALIVE_INTERVAL: u64 = 3;

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
    /// accept must be cancel safe
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

#[derive(Debug, Clone, Copy)]
struct Keepalive {
    // tcp_keepalive_time if the underlying protocol is TCP
    pub keepalive_secs: u64,
    // tcp_keepalive_intvl if the underlying protocol is TCP
    pub keepalive_interval: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct SocketOpts {
    // None means do not change
    nodelay: Option<bool>,
    // keepalive must be Some or None at the same time, or the behavior will be platform-dependent
    keepalive: Option<Keepalive>,
}

impl SocketOpts {
    fn none() -> SocketOpts {
        SocketOpts {
            nodelay: None,
            keepalive: None,
        }
    }

    /// Socket options for the control channel
    pub fn for_control_channel() -> SocketOpts {
        SocketOpts {
            nodelay: Some(true),  // Always set nodelay for the control channel
            ..SocketOpts::none()  // None means do not change. Keepalive is set by TcpTransport
        }
    }
}

impl SocketOpts {
    pub fn from_transport_cfg(cfg: &TransportConfig) -> SocketOpts {
        SocketOpts {
            nodelay: Some(cfg.nodelay),
            keepalive: Some(Keepalive {
                keepalive_secs: cfg.keepalive_secs,
                keepalive_interval: cfg.keepalive_interval,
            }),
        }
    }

    pub fn from_client_cfg(cfg: &ClientServiceConfig) -> SocketOpts {
        SocketOpts {
            nodelay: cfg.nodelay,
            ..SocketOpts::none()
        }
    }

    pub fn from_server_cfg(cfg: &ServerServiceConfig) -> SocketOpts {
        SocketOpts {
            nodelay: cfg.nodelay,
            ..SocketOpts::none()
        }
    }

    pub fn apply(&self, conn: &TcpStream) {
        if let Some(v) = self.keepalive {
            let keepalive_duration = Duration::from_secs(v.keepalive_secs);
            let keepalive_interval = Duration::from_secs(v.keepalive_interval);

            if let Err(e) = try_set_tcp_keepalive(conn, keepalive_duration, keepalive_interval)
                .with_context(|| "Failed to set keepalive")
            {
                error!("{:#}", e);
            }
        }

        if let Some(nodelay) = self.nodelay {
            trace!("Set nodelay {}", nodelay);
            if let Err(e) = conn
                .set_nodelay(nodelay)
                .with_context(|| "Failed to set nodelay")
            {
                error!("{:#}", e);
            }
        }
    }
}

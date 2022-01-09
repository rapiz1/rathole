use std::net::SocketAddr;

use super::Transport;
use crate::{
    config::{NoiseConfig, TransportConfig},
    helper::set_tcp_keepalive,
};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use snowstorm::{Builder, NoiseParams, NoiseStream};
use tokio::net::{TcpListener, TcpStream, ToSocketAddrs};
use crate::transport::{TransportStream, UnimplementedUnreliableStream};
use crate::transport::TransportStream::StrictlyReliable;

pub struct NoiseTransport {
    config: NoiseConfig,
    params: NoiseParams,
    local_private_key: Vec<u8>,
    remote_public_key: Option<Vec<u8>>,
}

impl std::fmt::Debug for NoiseTransport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> Result<(), std::fmt::Error> {
        write!(f, "{:?}", self.config)
    }
}

impl NoiseTransport {
    fn builder(&self) -> Builder {
        let builder = Builder::new(self.params.clone()).local_private_key(&self.local_private_key);
        match &self.remote_public_key {
            Some(x) => builder.remote_public_key(x),
            None => builder,
        }
    }
}

#[async_trait]
impl Transport for NoiseTransport {
    type Acceptor = TcpListener;
    type ReliableStream = snowstorm::stream::NoiseStream<TcpStream>;
    type UnreliableStream = UnimplementedUnreliableStream;
    type RawStream = TcpStream;

    async fn new(config: &TransportConfig) -> Result<Self> {
        let config = match &config.noise {
            Some(v) => v.clone(),
            None => return Err(anyhow!("Missing noise config")),
        };
        let builder = Builder::new(config.pattern.parse()?);

        let remote_public_key = match &config.remote_public_key {
            Some(x) => {
                Some(base64::decode(x).with_context(|| "Failed to decode remote_public_key")?)
            }
            None => None,
        };

        let local_private_key = match &config.local_private_key {
            Some(x) => base64::decode(x).with_context(|| "Failed to decode local_private_key")?,
            None => builder.generate_keypair()?.private,
        };

        let params: NoiseParams = config.pattern.parse()?;

        Ok(NoiseTransport {
            config,
            params,
            local_private_key,
            remote_public_key,
        })
    }

    async fn bind<T: ToSocketAddrs + Send + Sync>(&self, addr: T) -> Result<Self::Acceptor> {
        Ok(TcpListener::bind(addr).await?)
    }

    async fn accept(&self, a: &Self::Acceptor) -> Result<(Self::RawStream, SocketAddr)> {
        let (conn, addr) = a
            .accept()
            .await
            .with_context(|| "Failed to accept TCP connection")?;
        set_tcp_keepalive(&conn);
        Ok((conn, addr))
    }

    async fn handshake(&self, conn: Self::RawStream) -> Result<TransportStream<Self>> {
        let conn = NoiseStream::handshake(conn, self.builder().build_responder()?)
            .await
            .with_context(|| "Failed to do noise handshake")?;
        Ok(StrictlyReliable(conn))
    }

    async fn connect(&self, addr: &str) -> Result<TransportStream<Self>> {
        let conn = TcpStream::connect(addr)
            .await
            .with_context(|| "Failed to connect TCP socket")?;
        set_tcp_keepalive(&conn);

        let conn = NoiseStream::handshake(conn, self.builder().build_initiator()?)
            .await
            .with_context(|| "Failed to do noise handshake")?;
        return Ok(StrictlyReliable(conn));
    }
}

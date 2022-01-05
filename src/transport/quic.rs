use std::io::Error;
use std::net::{SocketAddr, UdpSocket};
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use anyhow::{anyhow, Context};
use async_trait::async_trait;
use futures_util::StreamExt;
use once_cell::sync::OnceCell;
use quinn::{
    ClientConfig, Endpoint, EndpointConfig, Incoming, NewConnection, RecvStream, SendStream,
    ServerConfig,
};
use rustls::{Certificate, PrivateKey};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::ToSocketAddrs;

use crate::config::TransportConfig;
use crate::transport::Transport;

#[derive(Debug)]
pub struct QuicTransport {
    rtt0: bool,
    native_roots: bool,
    server_config: OnceCell<ServerConfig>,
    client_config: OnceCell<ClientConfig>,
    cert: Option<Vec<rustls::Certificate>>,
    private_key: Option<rustls::PrivateKey>,
}

#[derive(Debug)]
pub struct QuicStream {
    #[allow(dead_code)]
    new_conn: NewConnection,
    bi: (SendStream, RecvStream),
    role: Role,
}

#[derive(Debug)]
pub struct QuicAccept(Endpoint, Incoming);

#[derive(Debug, Clone)]
enum Role {
    Server,
    Client(Endpoint),
}

#[async_trait]
impl Transport for QuicTransport {
    type Acceptor = QuicAccept;
    type Stream = QuicStream;

    async fn new(config: &TransportConfig) -> anyhow::Result<Self>
    where
        Self: Sized,
    {
        let quic_config = match &config.quic {
            Some(v) => v,
            None => {
                return Err(anyhow!("Missing quic config"));
            }
        };

        let key = if let Some(path) = &quic_config.private_key {
            let key = std::fs::read(path).context("failed to read private key")?;
            Some(
                if Path::new(path).extension().map_or(false, |x| x == "der") {
                    PrivateKey(key)
                } else {
                    let pkcs8 = rustls_pemfile::pkcs8_private_keys(&mut &*key)
                        .context("malformed PKCS #8 private key")?;
                    match pkcs8.into_iter().next() {
                        Some(x) => PrivateKey(x),
                        None => {
                            let rsa = rustls_pemfile::rsa_private_keys(&mut &*key)
                                .context("malformed PKCS #1 private key")?;
                            match rsa.into_iter().next() {
                                Some(x) => PrivateKey(x),
                                None => {
                                    anyhow::bail!("no private keys found");
                                }
                            }
                        }
                    }
                },
            )
        } else {
            None
        };

        let cert = if let Some(path) = &quic_config.cert {
            let cert = std::fs::read(path).context("failed to read certificate chain")?;
            Some(
                if Path::new(path).extension().map_or(false, |x| x == "der") {
                    vec![Certificate(cert)]
                } else {
                    rustls_pemfile::certs(&mut &*cert)
                        .context("invalid PEM-encoded certificate")?
                        .into_iter()
                        .map(Certificate)
                        .collect()
                },
            )
        } else {
            None
        };

        Ok(QuicTransport {
            cert,
            private_key: key,
            client_config: OnceCell::new(),
            server_config: OnceCell::new(),
            rtt0: quic_config.rtt0,
            native_roots: quic_config.native_roots,
        })
    }

    async fn bind<T: ToSocketAddrs + Send + Sync>(
        &self,
        addr: T,
    ) -> anyhow::Result<Self::Acceptor> {
        let socket = bind_udp_socket(addr).await?;
        let a = quinn::Endpoint::new(
            EndpointConfig::default(),
            Some(
                self.server_config
                    .get_or_try_init::<_, anyhow::Error>(|| {
                        // unwrap is ok, because config::validate_transport_config should be checked.
                        Ok(server_config(ServerConfig::with_single_cert(
                            self.cert.clone().unwrap(),
                            self.private_key.clone().unwrap(),
                        )?))
                    })?
                    .clone(),
            ),
            socket,
        )?;
        Ok(QuicAccept(a.0, a.1))
    }

    async fn accept(
        &self,
        QuicAccept(_, incoming): &mut Self::Acceptor,
    ) -> anyhow::Result<(Self::Stream, SocketAddr)> {
        let conn = incoming
            .next()
            .await
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::ConnectionReset))?;
        let peer_addr = conn.remote_address();
        let mut new_conn = if self.rtt0 {
            let (nc, z) = conn.into_0rtt().unwrap();
            z.await;
            nc
        } else {
            conn.await?
        };
        let bi = new_conn
            .bi_streams
            .next()
            .await
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::ConnectionReset))??;
        Ok((
            QuicStream {
                new_conn,
                bi,
                role: Role::Server,
            },
            peer_addr,
        ))
    }

    async fn connect(&self, addr: &str) -> anyhow::Result<Self::Stream> {
        let socket = bind_udp_socket("[::]:0").await?;
        let ip = tokio::net::lookup_host(addr)
            .await?
            .next()
            .ok_or_else(|| anyhow!("couldn't resolve to an address"))?;
        let ep = quinn::Endpoint::new(EndpointConfig::default(), None, socket)?.0;
        let connection = ep.connect_with(
            self.client_config
                .get_or_try_init::<_, anyhow::Error>(|| {
                    let mut client_config = if !self.native_roots {
                        let mut root = rustls::RootCertStore::empty();
                        if let Some(cert) = &self.cert {
                            cert.iter().try_for_each::<_, anyhow::Result<_>>(|cert| {
                                root.add(cert)?;
                                Ok(())
                            })?;
                        }
                        ClientConfig::with_root_certificates(root)
                    } else {
                        ClientConfig::with_native_roots()
                    };
                    client_config.transport = Arc::new(transport_config());
                    Ok(client_config)
                })?
                .clone(),
            ip,
            addr.split_once(":").map(|(s, _)| s).unwrap_or(addr),
        )?;

        let new_conn = if self.rtt0 {
            match connection.into_0rtt() {
                Ok((nc, z)) => {
                    z.await;
                    nc
                }
                Err(c) => c.await?,
            }
        } else {
            connection.await?
        };

        let bi = new_conn.connection.open_bi().await?;
        Ok(QuicStream {
            new_conn,
            bi,
            role: Role::Client(ep),
        })
    }
}

impl AsyncRead for QuicStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        Pin::new(&mut self.get_mut().bi.1).poll_read(cx, buf)
    }
}

impl AsyncWrite for QuicStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, Error>> {
        Pin::new(&mut self.get_mut().bi.0).poll_write(cx, buf)
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), Error>> {
        Pin::new(&mut self.get_mut().bi.0).poll_flush(cx)
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<Result<(), Error>> {
        Pin::new(&mut self.get_mut().bi.0).poll_shutdown(cx)
    }
}

impl Drop for QuicAccept {
    fn drop(&mut self) {
        self.0.close(0u8.into(), &[]);
    }
}

impl Drop for QuicStream {
    fn drop(&mut self) {
        if let Role::Client(e) = &self.role {
            e.close(0u8.into(), &[])
        }
    }
}

#[inline]
async fn bind_udp_socket<A>(addr: A) -> anyhow::Result<UdpSocket>
where
    A: ToSocketAddrs,
{
    tokio::net::UdpSocket::bind(addr)
        .await?
        .into_std()
        .map_err(Into::into)
}

fn server_config(mut config: ServerConfig) -> ServerConfig {
    config.transport = Arc::new(transport_config());
    config.migration(true);
    config
}

fn transport_config() -> quinn::TransportConfig {
    let mut transport_config = quinn::TransportConfig::default();
    transport_config
        .congestion_controller_factory(Arc::new(quinn::congestion::BbrConfig::default()));
    transport_config.max_idle_timeout(Some(Duration::from_secs(60).try_into().unwrap()));
    transport_config.keep_alive_interval(Some(Duration::from_secs(10)));

    transport_config
}

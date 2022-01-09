use futures::lock::Mutex;
use std::borrow::{Borrow, BorrowMut};
use std::fmt::{Debug, Formatter};
use std::io;
use std::io::{Error, IoSlice};
use std::net::SocketAddr;
use std::ops::Deref;
use std::ops::DerefMut;
use std::pin::Pin;
use std::sync::Arc;
use std::task::Poll;
use std::time::Duration;

use super::Transport;
use crate::config::{TlsConfig, TransportConfig};
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use bytes::Bytes;
use futures_util::{AsyncWriteExt, ready, StreamExt, Stream};
use openssl::pkcs12::Pkcs12;
use quinn::{Connection, ConnectionError, Datagrams, Endpoint, EndpointConfig, Incoming, NewConnection, SendDatagramError};
use rustls::internal::msgs::codec::Codec;
use rustls::ClientConfig;
use tokio::fs;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::{ToSocketAddrs, UdpSocket};
use tokio_native_tls::native_tls::Certificate;
use crate::transport::TransportStream;

pub const ALPN_QUIC_TUNNEL: &[&[u8]] = &[b"qt"];
pub const NONE: &str = "None";

pub struct QuicTransport {
    config: TlsConfig,
    client_crypto: Option<rustls::ClientConfig>,
}

impl Debug for QuicTransport {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        let client_crypto = &self.client_crypto.as_ref().map(|_| "ClientConfig{}");

        f.debug_struct("QuicTransport")
            .field("config", &self.config)
            .field("client_crypto", client_crypto)
            .finish()
    }
}

#[derive(Debug)]
pub struct QuicBiStream {
    send: quinn::SendStream,
    recv: quinn::RecvStream,
    conn: Connection,
}

#[derive(Debug)]
pub struct QuicDatagramStream {
    conn: Connection,
    datagrams: Datagrams,
}

impl QuicBiStream {
    fn new((mut send, recv): (quinn::SendStream, quinn::RecvStream), conn: Connection) -> Self {
        Self { send, recv, conn}
    }
}


impl QuicDatagramStream {
    fn new(mut datagrams: Datagrams, conn: Connection) -> Self {
        QuicDatagramStream{
            datagrams,
            conn,
        }
    }
}

impl tokio::io::AsyncRead for QuicBiStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        // ready!(AsyncRead::poll_read(self.recv., cx, buf))?;
        // Poll::Ready(Ok(()))
        Pin::new(self.get_mut().recv.borrow_mut()).poll_read(cx, buf)
    }
}

impl tokio::io::AsyncRead for QuicDatagramStream {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match Pin::new(self.get_mut().datagrams.borrow_mut()).poll_next(cx) {
            Poll::Ready(Some(Ok(b))) => {
                buf.put_slice(b.as_ref());
                return Poll::Ready(std::io::Result::Ok(()))
            }
            Poll::Ready(Some(Err(err))) => Poll::Ready(std::io::Result::Err(Error::from(std::io::ErrorKind::BrokenPipe))),
            Poll::Ready(None) =>  Poll::Ready(std::io::Result::Err(Error::from(std::io::ErrorKind::BrokenPipe))),
            Poll::Pending => {Poll::Pending}
        }
    }
}

impl AsyncWrite for QuicBiStream {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> Poll<std::result::Result<usize, Error>> {
        Pin::new(self.get_mut().send.borrow_mut()).poll_write(cx, buf)
    }

    fn poll_flush(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<std::result::Result<(), Error>> {
        Pin::new(self.get_mut().send.borrow_mut()).poll_flush(cx)
    }

    fn poll_shutdown(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> Poll<std::result::Result<(), Error>> {
        Pin::new(self.get_mut().send.borrow_mut()).poll_shutdown(cx)
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        bufs: &[IoSlice<'_>],
    ) -> Poll<std::result::Result<usize, Error>> {
        Pin::new(self.get_mut().send.borrow_mut()).poll_write_vectored(cx, bufs)
    }

    fn is_write_vectored(&self) -> bool {
        self.send.is_write_vectored()
    }
}

impl AsyncWrite for QuicDatagramStream {
    fn poll_write(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>, buf: &[u8]) -> Poll<std::result::Result<usize, Error>> {
        match self.conn.send_datagram(Bytes::from(buf)) {
            Ok(_) => Poll::Ready(Ok(buf.len())),
            Err(e) => Poll::Ready(std::io::Result::Err(Error::from(std::io::ErrorKind::BrokenPipe))),
            }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<std::result::Result<(), Error>> {
        todo!()
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<std::result::Result<(), Error>> {
        todo!()
    }
}


pub struct QuicAcceptor(Arc<Mutex<(Endpoint, Incoming)>>);

impl Drop for QuicAcceptor {
    fn drop(&mut self) {
        if let Some(guard) = self.0.try_lock() {
            guard.0.close(0u8.into(), &[])
        }
    }
}

impl Drop for QuicBiStream {
    fn drop(&mut self) {
        self.conn.close(0u8.into(), &[]);
    }
}

#[async_trait]
impl Transport for QuicTransport {
    type Acceptor = QuicAcceptor;
    type ReliableStream = QuicBiStream;
    type UnreliableStream = QuicDatagramStream;

    async fn new(config: &TransportConfig) -> Result<Self> {
        let config = match &config.tls {
            Some(v) => v,
            None => {
                return Err(anyhow!("Missing tls config"));
            }
        };

        let client_crypto = match config.trusted_root.as_ref() {
            Some(path) => {
                let s = fs::read_to_string(path)
                    .await
                    .with_context(|| "Failed to read the `tls.trusted_root`")?;
                let cert = Certificate::from_pem(s.as_bytes())
                    .with_context(|| "Failed to read certificate from `tls.trusted_root`")?;

                let mut roots = rustls::RootCertStore::empty();

                roots.add(&rustls::Certificate(
                    cert.to_der()
                        .with_context(|| "could not encode trust root as DER")?,
                ));

                let mut client_crypto = rustls::ClientConfig::builder()
                    .with_safe_defaults()
                    .with_root_certificates(roots)
                    .with_no_client_auth();
                client_crypto.alpn_protocols = ALPN_QUIC_TUNNEL.iter().map(|&x| x.into()).collect();
                Some(client_crypto)
            }
            None => None,
        };

        Ok(QuicTransport {
            config: config.clone(),
            client_crypto,
        })
    }

    async fn bind<A: ToSocketAddrs + Send + Sync>(&self, addr: A) -> Result<Self::Acceptor> {
        let buf = fs::read(self.config.pkcs12.as_ref().unwrap())
            .await
            .with_context(|| "Failed to read the `tls.pkcs12`")?;

        let pkcs12 =
            Pkcs12::from_der(buf.as_slice()).with_context(|| "Failed to open `tls.pkcs12`")?;

        let parsed = pkcs12
            .parse(self.config.pkcs12_password.as_ref().unwrap())
            .with_context(|| "Could not decrypt  `tls.pkcs12` using `tls.pkcs12_password`")?;

        let mut chain: Vec<rustls::Certificate> = parsed
            .chain
            .unwrap()
            .into_iter()
            .map(|cert| rustls::Certificate(cert.to_der().unwrap()))
            .rev()
            .collect();
        chain.insert(
            0,
            rustls::Certificate(
                parsed
                    .cert
                    .to_der()
                    .with_context(|| "Could not encode server cert as PEM")?,
            ),
        );

        let key = rustls::PrivateKey(parsed.pkey.private_key_to_der().unwrap());

        let mut server_crypto = rustls::ServerConfig::builder()
            .with_safe_defaults()
            .with_no_client_auth()
            .with_single_cert(chain, key)
            .with_context(|| "Server keys invalid")?;

        server_crypto.alpn_protocols = ALPN_QUIC_TUNNEL.iter().map(|&x| x.into()).collect();

        let mut server_config = quinn::ServerConfig::with_crypto(Arc::new(server_crypto));
        Arc::get_mut(&mut server_config.transport)
            .unwrap()
            .datagram_receive_buffer_size(Some(65536))
            .datagram_send_buffer_size(65536)
            .max_idle_timeout(Some(Duration::from_secs(10).try_into()?));

        server_config.use_retry(true);
        let socket = UdpSocket::bind(addr).await?.into_std()?;
        quinn::Endpoint::new(EndpointConfig::default(), Some(server_config), socket)
            .with_context(|| "Failed to start server")
            .map(|e_i| QuicAcceptor(Arc::new(Mutex::new(e_i))))
    }

    async fn accept(&self, a: &Self::Acceptor) -> Result<(TransportStream<Self>, SocketAddr)> {
        // let a_guard = a.lock().unwrap();
        while let Some(connecting) = a.0.lock().await.1.next().await {
            let addr = connecting.remote_address();
            let mut conn = connecting.await?;
            if let Some(stream) = conn.bi_streams.next().await {

                return Ok((TransportStream::PartiallyReliable(
                    QuicBiStream::new(stream.unwrap(), conn.connection.clone()),
                    QuicDatagramStream::new(conn.datagrams, conn.connection)
                ), addr));
            }
        }
        Err(anyhow!("endpoint closed"))
    }

    async fn connect(&self, addr: &str) -> Result<TransportStream<Self>> {
        let mut endpoint = quinn::Endpoint::client("[::]:0".parse().unwrap())
            .with_context(|| "could not open client socket")?;
        let mut config =
            quinn::ClientConfig::new(Arc::new(self.client_crypto.as_ref().unwrap().clone()));
        // server times out afte 10 sec, so send keepalive every 5 sec
        Arc::get_mut(&mut config.transport)
            .unwrap()
            .keep_alive_interval(Some(Duration::from_secs(5).try_into()?))
            .datagram_receive_buffer_size(Some(65536))
            .datagram_send_buffer_size(65536);
        endpoint.set_default_client_config(config);
        let connecting = endpoint.connect(
            addr.parse().with_context(|| "server address not valid")?,
            self.config
                .hostname
                .as_ref()
                .unwrap_or(&String::from(addr.split(':').next().unwrap())),
        )?;
        let new_conn = connecting.await?;

        Ok(TransportStream::PartiallyReliable(
            QuicBiStream::new(new_conn.connection.open_bi().await?, new_conn.connection.clone()),
            QuicDatagramStream::new(new_conn.datagrams, new_conn.connection),
        ))
    }
}


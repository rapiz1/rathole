use std::net::SocketAddr;
use std::sync::Arc;
use std::{fmt, fs};

use super::{SocketOpts, TcpTransport, Transport};
use crate::config::{TlsConfig, TransportConfig};
use crate::helper::host_port_pair;
use anyhow::{anyhow, Context, Result};
use async_trait::async_trait;
use p12::PFX;
use tokio::net::{TcpListener, TcpStream, ToSocketAddrs};
use tokio_rustls::rustls::{
    Certificate, ClientConfig, PrivateKey, RootCertStore, ServerConfig, ServerName,
};
use tokio_rustls::{TlsAcceptor, TlsConnector, TlsStream};

pub struct TlsTransport {
    tcp: TcpTransport,
    config: TlsConfig,
    connector: Option<TlsConnector>,
    tls_acceptor: Option<TlsAcceptor>,
}

impl fmt::Debug for TlsTransport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
        f.debug_struct("TlsTransport")
            .field("tcp", &self.tcp)
            .field("config", &self.config)
            .finish()
    }
}

fn load_server_config(config: &TlsConfig) -> Result<Option<ServerConfig>> {
    if let Some(pkcs12_path) = config.pkcs12.as_ref() {
        // TODO: with context
        let buf = fs::read(pkcs12_path)?;
        let pfx = PFX::parse(buf.as_slice())?;
        let pass = config.pkcs12_password.as_ref().unwrap();

        let keys = pfx.key_bags(pass)?;
        let certs = pfx.cert_bags(pass)?;

        let chain: Vec<Certificate> = certs.into_iter().map(Certificate).collect();
        let key = PrivateKey(keys.into_iter().next().unwrap());

        Ok(Some(
            ServerConfig::builder()
                .with_safe_defaults()
                .with_no_client_auth()
                .with_single_cert(chain, key)
                .unwrap(),
        ))
    } else {
        Ok(None)
    }
}

fn load_client_config(config: &TlsConfig) -> Result<Option<ClientConfig>> {
    if let Some(path) = config.trusted_root.as_ref() {
        let s =
            fs::read_to_string(path).with_context(|| "Failed to read the `tls.trusted_root`")?;
        let cert = Certificate(s.as_bytes().to_owned());

        let mut root_certs = RootCertStore::empty();
        root_certs.add(&cert)?;

        Ok(Some(
            ClientConfig::builder()
                .with_safe_defaults()
                .with_root_certificates(root_certs)
                .with_no_client_auth(),
        ))
    } else {
        Ok(None)
    }
}

#[async_trait]
impl Transport for TlsTransport {
    type Acceptor = TcpListener;
    type RawStream = TcpStream;
    type Stream = TlsStream<TcpStream>;

    fn new(config: &TransportConfig) -> Result<Self> {
        let tcp = TcpTransport::new(config)?;
        let config = config
            .tls
            .as_ref()
            .ok_or_else(|| anyhow!("Missing tls config"))?;

        let connector = load_client_config(config)?.map(|c| Arc::new(c).into());
        let tls_acceptor = load_server_config(config)?.map(|c| Arc::new(c).into());

        Ok(TlsTransport {
            tcp,
            config: config.clone(),
            connector,
            tls_acceptor,
        })
    }

    fn hint(conn: &Self::Stream, opt: SocketOpts) {
        let (s, _) = conn.get_ref();
        opt.apply(s);
    }

    async fn bind<A: ToSocketAddrs + Send + Sync>(&self, addr: A) -> Result<Self::Acceptor> {
        let l = TcpListener::bind(addr)
            .await
            .with_context(|| "Failed to create tcp listener")?;
        Ok(l)
    }

    async fn accept(&self, a: &Self::Acceptor) -> Result<(Self::RawStream, SocketAddr)> {
        self.tcp
            .accept(a)
            .await
            .with_context(|| "Failed to accept TCP connection")
    }

    //TODO:
    async fn handshake(&self, conn: Self::RawStream) -> Result<Self::Stream> {
        let conn = self.tls_acceptor.as_ref().unwrap().accept(conn).await?;
        Ok(TlsStream::Server(conn))
    }

    async fn connect(&self, addr: &str) -> Result<Self::Stream> {
        let conn = self.tcp.connect(addr).await?;

        let connector = self.connector.as_ref().unwrap();

        let hostname = self
            .config
            .hostname
            .as_deref()
            .unwrap_or(host_port_pair(addr)?.0);
        Ok(TlsStream::Client(
            connector
                .connect(ServerName::try_from(hostname)?, conn)
                .await?,
        ))
    }
}

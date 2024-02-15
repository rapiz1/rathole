#[cfg(all(feature = "native-tls-support", feature = "rustls-support"))]
compile_error!("Only one of `native-tls-support` and `rustls-support` can be enabled");

#[cfg(all(not(feature = "native-tls-support"), not(feature = "rustls-support")))]
compile_error!("Either `native-tls-support` or `rustls-support` must be enabled");

#[cfg(feature = "native-tls-support")]
mod native_tls_support {
    use crate::config::{TlsConfig, TransportConfig};
    use crate::helper::host_port_pair;
    use crate::transport::{AddrMaybeCached, SocketOpts, TcpTransport, Transport};
    use anyhow::{anyhow, Context, Result};
    use async_trait::async_trait;
    use std::fs;
    use std::net::SocketAddr;
    use tokio::net::{TcpListener, TcpStream, ToSocketAddrs};
    use tokio_native_tls::native_tls::{self, Certificate, Identity};
    use tokio_native_tls::{TlsAcceptor, TlsConnector, TlsStream};

    #[derive(Debug)]
    pub struct TlsTransport {
        tcp: TcpTransport,
        config: TlsConfig,
        connector: Option<TlsConnector>,
        tls_acceptor: Option<TlsAcceptor>,
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

            let connector = match config.trusted_root.as_ref() {
                Some(path) => {
                    let s = fs::read_to_string(path)
                        .with_context(|| "Failed to read the `tls.trusted_root`")?;
                    let cert = Certificate::from_pem(s.as_bytes())
                        .with_context(|| "Failed to read certificate from `tls.trusted_root`")?;
                    let connector = native_tls::TlsConnector::builder()
                        .add_root_certificate(cert)
                        .build()?;
                    Some(TlsConnector::from(connector))
                }
                None => {
                    // if no trusted_root is specified, allow TlsConnector to use system default
                    let connector = native_tls::TlsConnector::builder().build()?;
                    Some(TlsConnector::from(connector))
                }
            };

            let tls_acceptor = match config.pkcs12.as_ref() {
                Some(path) => {
                    let ident = Identity::from_pkcs12(
                        &fs::read(path)?,
                        config.pkcs12_password.as_ref().unwrap(),
                    )
                    .with_context(|| "Failed to create identitiy")?;
                    Some(TlsAcceptor::from(
                        native_tls::TlsAcceptor::new(ident).unwrap(),
                    ))
                }
                None => None,
            };

            Ok(TlsTransport {
                tcp,
                config: config.clone(),
                connector,
                tls_acceptor,
            })
        }

        fn hint(conn: &Self::Stream, opt: SocketOpts) {
            opt.apply(conn.get_ref().get_ref().get_ref());
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

        async fn handshake(&self, conn: Self::RawStream) -> Result<Self::Stream> {
            let conn = self.tls_acceptor.as_ref().unwrap().accept(conn).await?;
            Ok(conn)
        }

        async fn connect(&self, addr: &AddrMaybeCached) -> Result<Self::Stream> {
            let conn = self.tcp.connect(addr).await?;

            let connector = self.connector.as_ref().unwrap();
            Ok(connector
                .connect(
                    self.config
                        .hostname
                        .as_deref()
                        .unwrap_or(host_port_pair(&addr.addr)?.0),
                    conn,
                )
                .await?)
        }
    }
}

#[cfg(feature = "native-tls-support")]
pub(crate) use native_tls_support::TlsTransport;

#[cfg(feature = "rustls-support")]
pub(crate) mod ruslts_support {
    use crate::config::{TlsConfig, TransportConfig};
    use crate::helper::host_port_pair;
    use crate::transport::{AddrMaybeCached, SocketOpts, TcpTransport, Transport};
    use std::fmt::Debug;
    use std::fs;
    use std::net::SocketAddr;
    use std::sync::Arc;
    use tokio::net::{TcpListener, TcpStream, ToSocketAddrs};
    use tokio_rustls::rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName};

    use anyhow::{anyhow, Context, Result};
    use async_trait::async_trait;
    use p12::PFX;
    use tokio_rustls::rustls::{ClientConfig, RootCertStore, ServerConfig};
    use tokio_rustls::{TlsAcceptor, TlsConnector, TlsStream};

    pub struct TlsTransport {
        tcp: TcpTransport,
        config: TlsConfig,
        connector: Option<TlsConnector>,
        tls_acceptor: Option<TlsAcceptor>,
    }

    // workaround for TlsConnector and TlsAcceptor not implementing Debug
    impl Debug for TlsTransport {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.debug_struct("TlsTransport")
                .field("tcp", &self.tcp)
                .field("config", &self.config)
                .finish()
        }
    }

    fn load_server_config(config: &TlsConfig) -> Result<Option<ServerConfig>> {
        if let Some(pkcs12_path) = config.pkcs12.as_ref() {
            let buf = fs::read(pkcs12_path)?;
            let pfx = PFX::parse(buf.as_slice())?;
            let pass = config.pkcs12_password.as_ref().unwrap();

            let certs = pfx.cert_bags(pass)?;
            let keys = pfx.key_bags(pass)?;

            let chain: Vec<CertificateDer> = certs.into_iter().map(CertificateDer::from).collect();
            let key = PrivatePkcs8KeyDer::from(keys.into_iter().next().unwrap());

            Ok(Some(
                ServerConfig::builder()
                    .with_no_client_auth()
                    .with_single_cert(chain, key.into())?,
            ))
        } else {
            Ok(None)
        }
    }

    fn load_client_config(config: &TlsConfig) -> Result<Option<ClientConfig>> {
        let cert = if let Some(path) = config.trusted_root.as_ref() {
            rustls_pemfile::certs(&mut std::io::BufReader::new(fs::File::open(path).unwrap()))
                .map(|cert| cert.unwrap())
                .next()
                .with_context(|| "Failed to read certificate")?
        } else {
            // read from native
            match rustls_native_certs::load_native_certs() {
                Ok(certs) => certs.into_iter().next().unwrap(),
                Err(e) => {
                    eprintln!("Failed to load native certs: {}", e);
                    return Ok(None);
                }
            }
        };

        let mut root_certs = RootCertStore::empty();
        root_certs.add(cert).unwrap();

        Ok(Some(
            ClientConfig::builder()
                .with_root_certificates(root_certs)
                .with_no_client_auth(),
        ))
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

            let connector = load_client_config(config)
                .unwrap()
                .map(|c| Arc::new(c).into());
            let tls_acceptor = load_server_config(config)
                .unwrap()
                .map(|c| Arc::new(c).into());

            Ok(TlsTransport {
                tcp,
                config: config.clone(),
                connector,
                tls_acceptor,
            })
        }

        fn hint(conn: &Self::Stream, opt: SocketOpts) {
            opt.apply(conn.get_ref().0);
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

        async fn handshake(&self, conn: Self::RawStream) -> Result<Self::Stream> {
            let conn = self.tls_acceptor.as_ref().unwrap().accept(conn).await?;
            Ok(tokio_rustls::TlsStream::Server(conn))
        }

        async fn connect(&self, addr: &AddrMaybeCached) -> Result<Self::Stream> {
            let conn = self.tcp.connect(addr).await?;

            let connector = self.connector.as_ref().unwrap();

            let host_name = self
                .config
                .hostname
                .as_deref()
                .unwrap_or(host_port_pair(&addr.addr)?.0);

            Ok(tokio_rustls::TlsStream::Client(
                connector
                    .connect(ServerName::try_from(host_name)?.to_owned(), conn)
                    .await?,
            ))
        }
    }
}

#[cfg(feature = "rustls-support")]
pub(crate) use ruslts_support::TlsTransport;

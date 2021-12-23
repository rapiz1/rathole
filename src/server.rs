use crate::config::{Config, ServerConfig, ServerServiceConfig, ServiceType, TransportType};
use crate::constants::{listen_backoff, UDP_BUFFER_SIZE};
use crate::multi_map::MultiMap;
use crate::protocol::Hello::{ControlChannelHello, DataChannelHello};
use crate::protocol::{
    self, read_auth, read_hello, Ack, ControlChannelCmd, DataChannelCmd, Hello, UdpTraffic,
    HASH_WIDTH_IN_BYTES,
};
#[cfg(feature = "tls")]
use crate::transport::TlsTransport;
use crate::transport::{NoiseTransport, TcpTransport, Transport};
use anyhow::{anyhow, bail, Context, Result};
use backoff::backoff::Backoff;
use backoff::ExponentialBackoff;

use rand::RngCore;
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{self, copy_bidirectional, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream, UdpSocket};
use tokio::sync::{broadcast, mpsc, RwLock};
use tokio::time;
use tracing::{debug, error, info, info_span, instrument, warn, Instrument, Span};

type ServiceDigest = protocol::Digest; // SHA256 of a service name
type Nonce = protocol::Digest; // Also called `session_key`

const TCP_POOL_SIZE: usize = 64; // The number of cached connections for TCP servies
const UDP_POOL_SIZE: usize = 2; // The number of cached connections for UDP services
const CHAN_SIZE: usize = 2048; // The capacity of various chans

// The entrypoint of running a server
pub async fn run_server(config: &Config, shutdown_rx: broadcast::Receiver<bool>) -> Result<()> {
    let config = match &config.server {
            Some(config) => config,
            None => {
                return Err(anyhow!("Try to run as a server, but the configuration is missing. Please add the `[server]` block"))
            }
        };

    //TODO: Maybe use a Box<dyn trait> here to reduce duplicated code
    match config.transport.transport_type {
        TransportType::Tcp => {
            let mut server = Server::<TcpTransport>::from(config).await?;
            server.run(shutdown_rx).await?;
        }
        TransportType::Tls => {
            #[cfg(feature = "tls")]
            {
                let mut server = Server::<TlsTransport>::from(config).await?;
                server.run(shutdown_rx).await?;
            }
            #[cfg(not(feature = "tls"))]
            crate::helper::feature_not_compile("tls")
        }
        TransportType::Noise => {
            let mut server = Server::<NoiseTransport>::from(config).await?;
            server.run(shutdown_rx).await?;
        }
    }

    Ok(())
}

// A hash map of ControlChannelHandles, indexed by ServiceDigest or Nonce
// See also MultiMap
type ControlChannelMap<T> = MultiMap<ServiceDigest, Nonce, ControlChannelHandle<T>>;

// Server holds all states of running a server
struct Server<'a, T: Transport> {
    // `[server]` config
    config: &'a ServerConfig,

    // TODO: Maybe the rwlock is unnecessary.
    // Keep it until the hot reloading feature is implemented
    // `[server.services]` config, indexed by ServiceDigest
    services: Arc<RwLock<HashMap<ServiceDigest, ServerServiceConfig>>>,
    // Collection of contorl channels
    control_channels: Arc<RwLock<ControlChannelMap<T>>>,
    // Wrapper around the transport layer
    transport: Arc<T>,
}

// Generate a hash map of services which is indexed by ServiceDigest
fn generate_service_hashmap(
    server_config: &ServerConfig,
) -> HashMap<ServiceDigest, ServerServiceConfig> {
    let mut ret = HashMap::new();
    for u in &server_config.services {
        ret.insert(protocol::digest(u.0.as_bytes()), (*u.1).clone());
    }
    ret
}

impl<'a, T: 'static + Transport> Server<'a, T> {
    // Create a server from `[server]`
    pub async fn from(config: &'a ServerConfig) -> Result<Server<'a, T>> {
        Ok(Server {
            config,
            services: Arc::new(RwLock::new(generate_service_hashmap(config))),
            control_channels: Arc::new(RwLock::new(ControlChannelMap::new())),
            transport: Arc::new(T::new(&config.transport).await?),
        })
    }

    // The entry point of Server
    pub async fn run(&mut self, mut shutdown_rx: broadcast::Receiver<bool>) -> Result<()> {
        // Listen at `server.bind_addr`
        let l = self
            .transport
            .bind(&self.config.bind_addr)
            .await
            .with_context(|| "Failed to listen at `server.bind_addr`")?;
        info!("Listening at {}", self.config.bind_addr);

        // Retry at least every 100ms
        let mut backoff = ExponentialBackoff {
            max_interval: Duration::from_millis(100),
            max_elapsed_time: None,
            ..Default::default()
        };

        // Wait for connections and shutdown signals
        loop {
            tokio::select! {
                // Wait for incoming control and data channels
                ret = self.transport.accept(&l) => {
                    match ret {
                        Err(err) => {
                            // Detects whether it's an IO error
                            if let Some(err) = err.downcast_ref::<io::Error>() {
                                // If it is an IO error, then it's possibly an
                                // EMFILE. So sleep for a while and retry
                                // TODO: Only sleep for EMFILE, ENFILE, ENOMEM, ENOBUFS
                                if let Some(d) = backoff.next_backoff() {
                                    error!("Failed to accept: {}. Retry in {:?}...", err, d);
                                    time::sleep(d).await;
                                } else {
                                    // This branch will never be executed according to the current retry policy
                                    error!("Too many retries. Aborting...");
                                    break;
                                }
                            }
                            // If it's not an IO error, then it comes from
                            // the transport layer, so just ignore it
                        }
                        Ok((conn, addr)) => {
                            backoff.reset();
                            debug!("Incomming connection from {}", addr);

                            let services = self.services.clone();
                            let control_channels = self.control_channels.clone();
                            tokio::spawn(async move {
                                if let Err(err) = handle_connection(conn, addr, services, control_channels).await.with_context(||"Failed to handle a connection to `server.bind_addr`") {
                                    error!("{:?}", err);
                                }
                            }.instrument(info_span!("handle_connection", %addr)));
                        }
                    }
                },
                // Wait for the shutdown signal
                _ = shutdown_rx.recv() => {
                    info!("Shuting down gracefully...");
                    break;
                }
            }
        }

        Ok(())
    }
}

// Handle connections to `server.bind_addr`
async fn handle_connection<T: 'static + Transport>(
    mut conn: T::Stream,
    addr: SocketAddr,
    services: Arc<RwLock<HashMap<ServiceDigest, ServerServiceConfig>>>,
    control_channels: Arc<RwLock<ControlChannelMap<T>>>,
) -> Result<()> {
    // Read hello
    let hello = read_hello(&mut conn).await?;
    match hello {
        ControlChannelHello(_, service_digest) => {
            do_control_channel_handshake(conn, addr, services, control_channels, service_digest)
                .await?;
        }
        DataChannelHello(_, nonce) => {
            do_data_channel_handshake(conn, control_channels, nonce).await?;
        }
    }
    Ok(())
}

async fn do_control_channel_handshake<T: 'static + Transport>(
    mut conn: T::Stream,
    addr: SocketAddr,
    services: Arc<RwLock<HashMap<ServiceDigest, ServerServiceConfig>>>,
    control_channels: Arc<RwLock<ControlChannelMap<T>>>,
    service_digest: ServiceDigest,
) -> Result<()> {
    info!("New control channel incomming from {}", addr);

    // Generate a nonce
    let mut nonce = vec![0u8; HASH_WIDTH_IN_BYTES];
    rand::thread_rng().fill_bytes(&mut nonce);

    // Send hello
    let hello_send = Hello::ControlChannelHello(
        protocol::CURRENT_PROTO_VRESION,
        nonce.clone().try_into().unwrap(),
    );
    conn.write_all(&bincode::serialize(&hello_send).unwrap())
        .await?;

    // Lookup the service
    let services_guard = services.read().await;
    let service_config = match services_guard.get(&service_digest) {
        Some(v) => v,
        None => {
            conn.write_all(&bincode::serialize(&Ack::ServiceNotExist).unwrap())
                .await?;
            bail!("No such a service {}", hex::encode(&service_digest));
        }
    };
    let service_name = &service_config.name;

    // Calculate the checksum
    let mut concat = Vec::from(service_config.token.as_ref().unwrap().as_bytes());
    concat.append(&mut nonce);

    // Read auth
    let protocol::Auth(d) = read_auth(&mut conn).await?;

    // Validate
    let session_key = protocol::digest(&concat);
    if session_key != d {
        conn.write_all(&bincode::serialize(&Ack::AuthFailed).unwrap())
            .await?;
        debug!(
            "Expect {}, but got {}",
            hex::encode(session_key),
            hex::encode(d)
        );
        bail!("Service {} failed the authentication", service_name);
    } else {
        let service_config = service_config.clone();
        // Drop the rwlock as soon as possible when we're done with it
        drop(services_guard);

        let mut h = control_channels.write().await;

        // If there's already a control channel for the service, then drop the old one.
        // Because a control channel doesn't report back when it's dead,
        // the handle in the map could be stall, dropping the old handle enables
        // the client to reconnect.
        if h.remove1(&service_digest).is_some() {
            warn!(
                "Dropping previous control channel for digest {}",
                hex::encode(service_digest)
            );
        }

        // Send ack
        conn.write_all(&bincode::serialize(&Ack::Ok).unwrap())
            .await?;

        info!(service = %service_config.name, "Control channel established");
        let handle = ControlChannelHandle::new(conn, service_config);

        // Insert the new handle
        let _ = h.insert(service_digest, session_key, handle);
    }

    Ok(())
}

async fn do_data_channel_handshake<T: 'static + Transport>(
    conn: T::Stream,
    control_channels: Arc<RwLock<ControlChannelMap<T>>>,
    nonce: Nonce,
) -> Result<()> {
    // Validate
    let control_channels_guard = control_channels.read().await;
    match control_channels_guard.get2(&nonce) {
        Some(handle) => {
            // Send the data channel to the corresponding control channel
            handle.data_ch_tx.send(conn).await?;
        }
        None => {
            // TODO: Maybe print IP here
            warn!("Data channel has incorrect nonce");
        }
    }
    Ok(())
}

pub struct ControlChannelHandle<T: Transport> {
    // Shutdown the control channel.
    // Not used for now, but can be used for hot reloading
    #[allow(dead_code)]
    shutdown_tx: broadcast::Sender<bool>,
    //data_ch_req_tx: mpsc::Sender<bool>,
    data_ch_tx: mpsc::Sender<T::Stream>,
}

impl<T> ControlChannelHandle<T>
where
    T: 'static + Transport,
{
    // Create a control channel handle, where the control channel handling task
    // and the connection pool task are created.
    #[instrument(skip_all, fields(service = %service.name))]
    fn new(conn: T::Stream, service: ServerServiceConfig) -> ControlChannelHandle<T> {
        // Save the name string for logging
        let name = service.name.clone();

        // Create a shutdown channel. The sender is not used for now, but for future use
        let (shutdown_tx, shutdown_rx) = broadcast::channel::<bool>(1);

        // Store data channels
        let (data_ch_tx, data_ch_rx) = mpsc::channel(CHAN_SIZE * 2);

        // Store data channel creation requests
        let (data_ch_req_tx, data_ch_req_rx) = mpsc::unbounded_channel();

        match service.service_type {
            ServiceType::Tcp => tokio::spawn(
                run_tcp_connection_pool::<T>(
                    service.bind_addr.clone(),
                    data_ch_rx,
                    data_ch_req_tx.clone(),
                    shutdown_tx.subscribe(),
                )
                .instrument(Span::current()),
            ),
            ServiceType::Udp => tokio::spawn(
                run_udp_connection_pool::<T>(
                    service.bind_addr.clone(),
                    data_ch_rx,
                    data_ch_req_tx.clone(),
                    shutdown_tx.subscribe(),
                )
                .instrument(Span::current()),
            ),
        };

        // Cache some data channels for later use
        let pool_size = match service.service_type {
            ServiceType::Tcp => TCP_POOL_SIZE,
            ServiceType::Udp => UDP_POOL_SIZE,
        };

        for _i in 0..pool_size {
            if let Err(e) = data_ch_req_tx.send(true) {
                error!("Failed to request data channel {}", e);
            };
        }

        // Create the control channel
        let ch = ControlChannel::<T> {
            conn,
            shutdown_rx,
            service,
            data_ch_req_rx,
        };

        // Run the control channel
        tokio::spawn(async move {
            if let Err(err) = ch.run().await {
                error!(%name, "{}", err);
            }
        });

        ControlChannelHandle {
            shutdown_tx,
            data_ch_tx,
        }
    }

    #[allow(dead_code)]
    fn shutdown(self) {
        let _ = self.shutdown_tx.send(true);
    }
}

// Control channel, using T as the transport layer. P is TcpStream or UdpTraffic
struct ControlChannel<T: Transport> {
    conn: T::Stream,                               // The connection of control channel
    service: ServerServiceConfig,                  // A copy of the corresponding service config
    shutdown_rx: broadcast::Receiver<bool>,        // Receives the shutdown signal
    data_ch_req_rx: mpsc::UnboundedReceiver<bool>, // Receives visitor connections
}

impl<T: Transport> ControlChannel<T> {
    // Run a control channel
    #[instrument(skip(self), fields(service = %self.service.name))]
    async fn run(mut self) -> Result<()> {
        let cmd = bincode::serialize(&ControlChannelCmd::CreateDataChannel).unwrap();

        // Wait for data channel requests and the shutdown signal
        loop {
            tokio::select! {
                val = self.data_ch_req_rx.recv() => {
                    match val {
                        Some(_) => {
                            if let Err(e) = self.conn.write_all(&cmd).await.with_context(||"Failed to write data cmds") {
                                error!("{:?}", e);
                                break;
                            }
                        }
                        None => {
                            break;
                        }
                    }
                },
                // Wait for the shutdown signal
                _ = self.shutdown_rx.recv() => {
                    break;
                }
            }
        }

        info!("Control channel shuting down");

        Ok(())
    }
}

fn tcp_listen_and_send(
    addr: String,
    data_ch_req_tx: mpsc::UnboundedSender<bool>,
    mut shutdown_rx: broadcast::Receiver<bool>,
) -> mpsc::Receiver<TcpStream> {
    let (tx, rx) = mpsc::channel(CHAN_SIZE);

    tokio::spawn(async move {
        let l = backoff::future::retry(listen_backoff(), || async {
            Ok(TcpListener::bind(&addr).await?)
        })
        .await
        .with_context(|| "Failed to listen for the service");

        let l: TcpListener = match l {
            Ok(v) => v,
            Err(e) => {
                error!("{:?}", e);
                return;
            }
        };

        info!("Listening at {}", &addr);

        // Retry at least every 1s
        let mut backoff = ExponentialBackoff {
            max_interval: Duration::from_secs(1),
            max_elapsed_time: None,
            ..Default::default()
        };

        // Wait for visitors and the shutdown signal
        loop {
            tokio::select! {
                val = l.accept() => {
                    match val {
                        Err(e) => {
                            // `l` is a TCP listener so this must be a IO error
                            // Possibly a EMFILE. So sleep for a while
                            error!("{}. Sleep for a while", e);
                            if let Some(d) = backoff.next_backoff() {
                                time::sleep(d).await;
                            } else {
                                // This branch will never be reached for current backoff policy
                                error!("Too many retries. Aborting...");
                                break;
                            }
                        }
                        Ok((incoming, addr)) => {
                            // For every visitor, request to create a data channel
                            if let Err(e) = data_ch_req_tx.send(true) {
                                // An error indicates the control channel is broken
                                // So break the loop
                                error!("{}", e);
                                break;
                            }

                            backoff.reset();

                            debug!("New visitor from {}", addr);

                            // Send the visitor to the connection pool
                            let _ = tx.send(incoming).await;
                        }
                    }
                },
                _ = shutdown_rx.recv() => {
                    break;
                }
            }
        }
    });

    rx
}

#[instrument(skip_all)]
async fn run_tcp_connection_pool<T: Transport>(
    bind_addr: String,
    mut data_ch_rx: mpsc::Receiver<T::Stream>,
    data_ch_req_tx: mpsc::UnboundedSender<bool>,
    shutdown_rx: broadcast::Receiver<bool>,
) -> Result<()> {
    let mut visitor_rx = tcp_listen_and_send(bind_addr, data_ch_req_tx, shutdown_rx);
    while let Some(mut visitor) = visitor_rx.recv().await {
        if let Some(mut ch) = data_ch_rx.recv().await {
            tokio::spawn(async move {
                let cmd = bincode::serialize(&DataChannelCmd::StartForwardTcp).unwrap();
                if ch.write_all(&cmd).await.is_ok() {
                    let _ = copy_bidirectional(&mut ch, &mut visitor).await;
                }
            });
        } else {
            break;
        }
    }
    Ok(())
}

#[instrument(skip_all)]
async fn run_udp_connection_pool<T: Transport>(
    bind_addr: String,
    mut data_ch_rx: mpsc::Receiver<T::Stream>,
    _data_ch_req_tx: mpsc::UnboundedSender<bool>,
    mut shutdown_rx: broadcast::Receiver<bool>,
) -> Result<()> {
    // TODO: Load balance

    let l: UdpSocket = backoff::future::retry(listen_backoff(), || async {
        Ok(UdpSocket::bind(&bind_addr).await?)
    })
    .await
    .with_context(|| "Failed to listen for the service")?;

    info!("Listening at {}", &bind_addr);

    let cmd = bincode::serialize(&DataChannelCmd::StartForwardUdp).unwrap();

    // Receive one data channel
    let mut conn = data_ch_rx
        .recv()
        .await
        .ok_or(anyhow!("No available data channels"))?;
    conn.write_all(&cmd).await?;

    let mut buf = [0u8; UDP_BUFFER_SIZE];
    loop {
        tokio::select! {
            // Forward inbound traffic to the client
            val = l.recv_from(&mut buf) => {
                let (n, from) = val?;
                UdpTraffic::write_slice(&mut conn, from, &buf[..n]).await?;
            },

            // Forward outbound traffic from the client to the visitor
            t = UdpTraffic::read(&mut conn) => {
                let t = t?;
                l.send_to(&t.data, t.from).await?;
            },

            _ = shutdown_rx.recv() => {
                break;
            }
        }
    }

    Ok(())
}

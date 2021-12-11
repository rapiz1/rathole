use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use crate::config::{Config, ServerConfig, ServerServiceConfig};
use crate::helper::set_tcp_keepalive;
use crate::multi_map::MultiMap;
use crate::protocol::{
    self, read_hello, Hello, Hello::ControlChannelHello, Hello::DataChannelHello,
};
use crate::protocol::{read_auth, Ack, ControlChannelCmd, DataChannelCmd, HASH_WIDTH_IN_BYTES};
use anyhow::{anyhow, bail, Context, Result};
use rand::RngCore;
use tokio::io::{self, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::sync::{oneshot, RwLock};
use tokio::time;
use tokio::{
    self,
    net::{self, TcpListener, TcpStream},
};
use tracing::{debug, error, info, info_span, warn, Instrument};

use backoff::{backoff::Backoff, ExponentialBackoff};

type ServiceDigest = protocol::Digest;
type Nonce = protocol::Digest;

const POOL_SIZE: usize = 64;
const CHAN_SIZE: usize = 2048;

pub async fn run_server(config: &Config) -> Result<()> {
    let mut server = Server::from(config)?;

    server.run().await
}

type ControlChannelMap = MultiMap<ServiceDigest, Nonce, ControlChannelHandle>;
struct Server<'a> {
    config: &'a ServerConfig,
    services: Arc<RwLock<HashMap<ServiceDigest, ServerServiceConfig>>>,
    control_channels: Arc<RwLock<ControlChannelMap>>,
}

impl<'a> Server<'a> {
    pub fn from(config: &'a Config) -> Result<Server> {
        match &config.server {
            Some(config) => Ok(Server {
                config,
                services: Arc::new(RwLock::new(Server::generate_service_hashmap(config))),
                control_channels: Arc::new(RwLock::new(ControlChannelMap::new())),
            }),
            None =>
            Err(anyhow!("Try to run as a server, but the configuration is missing. Please add the `[server]` block"))
        }
    }

    fn generate_service_hashmap(
        server_config: &ServerConfig,
    ) -> HashMap<ServiceDigest, ServerServiceConfig> {
        let mut ret = HashMap::new();
        for u in &server_config.services {
            ret.insert(protocol::digest(u.0.as_bytes()), (*u.1).clone());
        }
        ret
    }

    pub async fn run(&mut self) -> Result<()> {
        let l = net::TcpListener::bind(&self.config.bind_addr)
            .await
            .with_context(|| "Failed to listen at `server.bind_addr`")?;
        info!("Listening at {}", self.config.bind_addr);

        // Retry at least every 100ms
        let mut backoff = ExponentialBackoff {
            max_interval: Duration::from_millis(100),
            max_elapsed_time: None,
            ..Default::default()
        };

        // Listen for incoming control or data channels
        loop {
            tokio::select! {
                ret = l.accept() => {
                    match ret {
                        Err(err) => {
                            // Possibly a EMFILE. So sleep for a while and retry
                            if let Some(d) = backoff.next_backoff() {
                                error!("Failed to accept: {}. Retry in {:?}...", err, d);
                                time::sleep(d).await;
                            } else {
                                // This branch will never be executed according to the current retry policy
                                error!("Too many retries. Aborting...");
                                break;
                            }
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
                _ = tokio::signal::ctrl_c() => {
                    info!("Shuting down gracefully...");
                    break;
                }
            }
        }

        Ok(())
    }
}

async fn handle_connection(
    mut conn: TcpStream,
    addr: SocketAddr,
    services: Arc<RwLock<HashMap<ServiceDigest, ServerServiceConfig>>>,
    control_channels: Arc<RwLock<ControlChannelMap>>,
) -> Result<()> {
    // Read hello
    let hello = read_hello(&mut conn).await?;
    match hello {
        ControlChannelHello(_, service_digest) => {
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
            let d = match read_auth(&mut conn).await? {
                protocol::Auth(v) => v,
            };

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
                let mut h = control_channels.write().await;

                if let Some(_) = h.remove1(&service_digest) {
                    warn!(
                        "Dropping previous control channel for digest {}",
                        hex::encode(service_digest)
                    );
                }

                let service_config = service_config.clone();
                drop(services_guard);

                // Send ack
                conn.write_all(&bincode::serialize(&Ack::Ok).unwrap())
                    .await?;

                info!(service = %service_config.name, "Control channel established");
                let handle = ControlChannelHandle::new(conn, service_config);

                // Drop the old handle
                let _ = h.insert(service_digest, session_key, handle);
            }
        }
        DataChannelHello(_, nonce) => {
            // Validate
            let control_channels_guard = control_channels.read().await;
            match control_channels_guard.get2(&nonce) {
                Some(c_ch) => {
                    if let Err(e) = set_tcp_keepalive(&conn) {
                        error!("The connection may be unstable! {:?}", e);
                    }

                    // Send the data channel to the corresponding control channel
                    c_ch.conn_pool.data_ch_tx.send(conn).await?;
                }
                None => {
                    warn!("Data channel has incorrect nonce");
                }
            }
        }
    }
    Ok(())
}

struct ControlChannel {
    conn: TcpStream,
    service: ServerServiceConfig,
    shutdown_rx: oneshot::Receiver<bool>,
    visitor_tx: mpsc::Sender<TcpStream>,
}

struct ControlChannelHandle {
    shutdown_tx: oneshot::Sender<bool>,
    conn_pool: ConnectionPoolHandle,
}

impl ControlChannelHandle {
    fn new(conn: TcpStream, service: ServerServiceConfig) -> ControlChannelHandle {
        let (shutdown_tx, shutdown_rx) = oneshot::channel::<bool>();
        let name = service.name.clone();
        let conn_pool = ConnectionPoolHandle::new();
        let actor = ControlChannel {
            conn,
            shutdown_rx,
            service,
            visitor_tx: conn_pool.visitor_tx.clone(),
        };

        tokio::spawn(async move {
            if let Err(err) = actor.run().await {
                error!(%name, "{}", err);
            }
        });

        ControlChannelHandle {
            shutdown_tx,
            conn_pool,
        }
    }
}

impl ControlChannel {
    #[tracing::instrument(skip(self), fields(service = %self.service.name))]
    async fn run(mut self) -> Result<()> {
        if let Err(e) = set_tcp_keepalive(&self.conn) {
            error!("The connection may be unstable! {:?}", e);
        }

        let l = match TcpListener::bind(&self.service.bind_addr).await {
            Ok(v) => v,
            Err(e) => {
                let duration = Duration::from_secs(1);
                error!(
                    "Failed to listen on service.bind_addr: {}. Retry in {:?}...",
                    e, duration
                );
                time::sleep(duration).await;
                TcpListener::bind(&self.service.bind_addr).await?
            }
        };

        info!("Listening at {}", &self.service.bind_addr);

        let (data_req_tx, mut data_req_rx) = mpsc::unbounded_channel::<u8>();
        tokio::spawn(async move {
            let cmd = bincode::serialize(&ControlChannelCmd::CreateDataChannel).unwrap();
            while let Some(_) = data_req_rx.recv().await {
                if self.conn.write_all(&cmd).await.is_err() {
                    break;
                }
            }
        });

        for _i in 0..POOL_SIZE {
            if let Err(e) = data_req_tx.send(0) {
                error!("Failed to request data channel {}", e);
            };
        }

        let mut backoff = ExponentialBackoff {
            max_interval: Duration::from_secs(1),
            ..Default::default()
        };
        loop {
            tokio::select! {
                val = l.accept() => {
                    match val {
                        Err(e) => {
                            error!("{}. Sleep for a while", e);
                            if let Some(d) = backoff.next_backoff() {
                                time::sleep(d).await;
                            } else {
                                error!("Too many retries. Aborting...");
                                break;
                            }
                        },
                        Ok((incoming, addr)) => {
                            if let Err(e) = data_req_tx.send(0) {
                                error!("{}", e);
                                break;
                            };

                            backoff.reset();

                            debug!("New visitor from {}", addr);

                            let _ = self.visitor_tx.send(incoming).await;
                        }
                    }
                },
                _ = &mut self.shutdown_rx => {
                    break;
                }
            }
        }
        info!("Service shuting down");

        Ok(())
    }
}

#[derive(Debug)]
struct ConnectionPool {
    visitor_rx: mpsc::Receiver<TcpStream>,
    data_ch_rx: mpsc::Receiver<TcpStream>,
}

struct ConnectionPoolHandle {
    visitor_tx: mpsc::Sender<TcpStream>,
    data_ch_tx: mpsc::Sender<TcpStream>,
}

impl ConnectionPoolHandle {
    fn new() -> ConnectionPoolHandle {
        let (data_ch_tx, data_ch_rx) = mpsc::channel(CHAN_SIZE * 2);
        let (visitor_tx, visitor_rx) = mpsc::channel(CHAN_SIZE);
        let conn_pool = ConnectionPool {
            data_ch_rx,
            visitor_rx,
        };

        tokio::spawn(async move { conn_pool.run().await });

        ConnectionPoolHandle {
            data_ch_tx,
            visitor_tx,
        }
    }
}

impl ConnectionPool {
    #[tracing::instrument]
    async fn run(mut self) {
        loop {
            if let Some(mut visitor) = self.visitor_rx.recv().await {
                if let Some(mut ch) = self.data_ch_rx.recv().await {
                    tokio::spawn(async move {
                        let cmd = bincode::serialize(&DataChannelCmd::StartForward).unwrap();
                        if ch.write_all(&cmd).await.is_ok() {
                            let _ = io::copy_bidirectional(&mut ch, &mut visitor).await;
                        }
                    });
                } else {
                    break;
                }
            } else {
                break;
            }
        }
    }
}

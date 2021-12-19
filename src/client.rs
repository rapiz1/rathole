use std::collections::HashMap;
use std::sync::Arc;

use crate::config::{ClientConfig, ClientServiceConfig, Config, TransportType};
use crate::protocol::Hello::{self, *};
use crate::protocol::{
    self, read_ack, read_control_cmd, read_data_cmd, read_hello, Ack, Auth, ControlChannelCmd,
    DataChannelCmd, CURRENT_PROTO_VRESION, HASH_WIDTH_IN_BYTES,
};
use crate::transport::{TcpTransport, TlsTransport, Transport};
use anyhow::{anyhow, bail, Context, Result};
use backoff::ExponentialBackoff;

use tokio::io::{copy_bidirectional, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{broadcast, oneshot};
use tokio::time::{self, Duration};
use tracing::{debug, error, info, instrument, Instrument, Span};

// The entrypoint of running a client
pub async fn run_client(config: &Config, shutdown_rx: broadcast::Receiver<bool>) -> Result<()> {
    let config = match &config.client {
        Some(v) => v,
        None => {
            return Err(anyhow!("Try to run as a client, but the configuration is missing. Please add the `[client]` block"))
        }
    };

    match config.transport.transport_type {
        TransportType::Tcp => {
            let mut client = Client::<TcpTransport>::from(config).await?;
            client.run(shutdown_rx).await
        }
        TransportType::Tls => {
            let mut client = Client::<TlsTransport>::from(config).await?;
            client.run(shutdown_rx).await
        }
    }
}

type ServiceDigest = protocol::Digest;
type Nonce = protocol::Digest;

// Holds the state of a client
struct Client<'a, T: Transport> {
    config: &'a ClientConfig,
    service_handles: HashMap<String, ControlChannelHandle>,
    transport: Arc<T>,
}

impl<'a, T: 'static + Transport> Client<'a, T> {
    // Create a Client from `[client]` config block
    async fn from(config: &'a ClientConfig) -> Result<Client<'a, T>> {
        Ok(Client {
            config,
            service_handles: HashMap::new(),
            transport: Arc::new(
                *T::new(&config.transport)
                    .await
                    .with_context(|| "Failed to create the transport")?,
            ),
        })
    }

    // The entrypoint of Client
    async fn run(&mut self, mut shutdown_rx: broadcast::Receiver<bool>) -> Result<()> {
        for (name, config) in &self.config.services {
            // Create a control channel for each service defined
            let handle = ControlChannelHandle::new(
                (*config).clone(),
                self.config.remote_addr.clone(),
                self.transport.clone(),
            );
            self.service_handles.insert(name.clone(), handle);
        }

        // TODO: Maybe wait for a config change signal for hot reloading
        // Wait for the shutdown signal
        loop {
            tokio::select! {
                val = shutdown_rx.recv() => {
                    match val {
                        Ok(_) => {}
                        Err(err) => {
                            error!("Unable to listen for shutdown signal: {}", err);
                        }
                    }
                    break;
                },
            }
        }

        // Shutdown all services
        for (_, handle) in self.service_handles.drain() {
            handle.shutdown();
        }

        Ok(())
    }
}

struct RunDataChannelArgs<T: Transport> {
    session_key: Nonce,
    remote_addr: String,
    local_addr: String,
    connector: Arc<T>,
}

async fn run_data_channel<T: Transport>(args: Arc<RunDataChannelArgs<T>>) -> Result<()> {
    // Retry at least every 100ms, at most for 10 seconds
    let backoff = ExponentialBackoff {
        max_interval: Duration::from_millis(100),
        max_elapsed_time: Some(Duration::from_secs(10)),
        ..Default::default()
    };

    // Connect to remote_addr
    let mut conn: T::Stream = backoff::future::retry(backoff, || async {
        Ok(args
            .connector
            .connect(&args.remote_addr)
            .await
            .with_context(|| "Failed to connect to remote_addr")?)
    })
    .await?;

    // Send nonce
    let v: &[u8; HASH_WIDTH_IN_BYTES] = args.session_key[..].try_into().unwrap();
    let hello = Hello::DataChannelHello(CURRENT_PROTO_VRESION, v.to_owned());
    conn.write_all(&bincode::serialize(&hello).unwrap()).await?;

    // Forward
    match read_data_cmd(&mut conn).await? {
        DataChannelCmd::StartForward => {
            let mut local = TcpStream::connect(&args.local_addr)
                .await
                .with_context(|| "Failed to conenct to local_addr")?;
            let _ = copy_bidirectional(&mut conn, &mut local).await;
        }
    }
    Ok(())
}

// Control channel, using T as the transport layer
struct ControlChannel<T: Transport> {
    digest: ServiceDigest,              // SHA256 of the service name
    service: ClientServiceConfig,       // `[client.services.foo]` config block
    shutdown_rx: oneshot::Receiver<u8>, // Receives the shutdown signal
    remote_addr: String,                // `client.remote_addr`
    transport: Arc<T>,                  // Wrapper around the transport layer
}

// Handle of a control channel
// Dropping it will also drop the actual control channel
struct ControlChannelHandle {
    shutdown_tx: oneshot::Sender<u8>,
}

impl<T: 'static + Transport> ControlChannel<T> {
    #[instrument(skip(self), fields(service=%self.service.name))]
    async fn run(&mut self) -> Result<()> {
        let mut conn = self
            .transport
            .connect(&self.remote_addr)
            .await
            .with_context(|| format!("Failed to connect to the server: {}", &self.remote_addr))?;

        // Send hello
        let hello_send =
            Hello::ControlChannelHello(CURRENT_PROTO_VRESION, self.digest[..].try_into().unwrap());
        conn.write_all(&bincode::serialize(&hello_send).unwrap())
            .await?;

        // Read hello))
        let nonce = match read_hello(&mut conn)
            .await
            .with_context(|| "Failed to read hello from the server")?
        {
            ControlChannelHello(_, d) => d,
            _ => {
                bail!("Unexpected type of hello");
            }
        };

        // Send auth
        let mut concat = Vec::from(self.service.token.as_ref().unwrap().as_bytes());
        concat.extend_from_slice(&nonce);

        let session_key = protocol::digest(&concat);
        let auth = Auth(session_key);
        conn.write_all(&bincode::serialize(&auth).unwrap()).await?;

        // Read ack
        match read_ack(&mut conn).await? {
            Ack::Ok => {}
            v => {
                return Err(anyhow!("{}", v))
                    .with_context(|| format!("Authentication failed: {}", self.service.name));
            }
        }

        // Channel ready
        info!("Control channel established");

        let remote_addr = self.remote_addr.clone();
        let local_addr = self.service.local_addr.clone();
        let data_ch_args = Arc::new(RunDataChannelArgs {
            session_key,
            remote_addr,
            local_addr,
            connector: self.transport.clone(),
        });

        loop {
            tokio::select! {
                val = read_control_cmd(&mut conn) => {
                    let val = val?;
                    debug!( "Received {:?}", val);
                    match val {
                        ControlChannelCmd::CreateDataChannel => {
                            let args = data_ch_args.clone();
                            tokio::spawn(async move {
                                if let Err(e) = run_data_channel(args).await.with_context(|| "Failed to run the data channel") {
                                    error!("{:?}", e);
                                }
                            }.instrument(Span::current()));
                        }
                    }
                },
                _ = &mut self.shutdown_rx => {
                    info!( "Shutting down gracefully...");
                    break;
                }
            }
        }

        Ok(())
    }
}

impl ControlChannelHandle {
    #[instrument(skip_all, fields(service = %service.name))]
    fn new<T: 'static + Transport>(
        service: ClientServiceConfig,
        remote_addr: String,
        transport: Arc<T>,
    ) -> ControlChannelHandle {
        let digest = protocol::digest(service.name.as_bytes());
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let mut s = ControlChannel {
            digest,
            service,
            shutdown_rx,
            remote_addr,
            transport,
        };

        tokio::spawn(
            async move {
                while let Err(err) = s
                    .run()
                    .await
                    .with_context(|| "Failed to run the control channel")
                {
                    let duration = Duration::from_secs(1);
                    error!("{:?}\n\nRetry in {:?}...", err, duration);
                    time::sleep(duration).await;
                }
            }
            .instrument(Span::current()),
        );

        ControlChannelHandle { shutdown_tx }
    }

    fn shutdown(self) {
        // A send failure shows that the actor has already shutdown.
        let _ = self.shutdown_tx.send(0u8);
    }
}

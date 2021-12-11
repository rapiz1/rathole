use std::collections::HashMap;
use std::sync::Arc;

use crate::config::{ClientConfig, ClientServiceConfig, Config};
use crate::protocol::{
    self, read_hello, DataChannelCmd,
    Hello::{self, *},
    CURRENT_PROTO_VRESION, HASH_WIDTH_IN_BYTES,
};
use crate::protocol::{read_data_cmd, Ack, Auth, ControlChannelCmd};
use anyhow::{anyhow, bail, Context, Result};
use backoff::ExponentialBackoff;
use tokio::io;
use tokio::sync::oneshot;
use tokio::time::{self, Duration};
use tokio::{self, io::AsyncWriteExt, net::TcpStream};
use tracing::{debug, error, info, instrument, Instrument, Span};

pub async fn run_client(config: &Config) -> Result<()> {
    let mut client = Client::from(config)?;
    client.run().await
}

type ServiceDigest = protocol::Digest;
type Nonce = protocol::Digest;

struct Client<'a> {
    config: &'a ClientConfig,
    service_handles: HashMap<String, ControlChannelHandle>,
}

impl<'a> Client<'a> {
    fn from(config: &'a Config) -> Result<Client> {
        if let Some(config) = &config.client {
            Ok(Client {
                config,
                service_handles: HashMap::new(),
            })
        } else {
            Err(anyhow!("Try to run as a client, but the configuration is missing. Please add the `[client]` block"))
        }
    }

    async fn run(&mut self) -> Result<()> {
        for (name, config) in &self.config.services {
            let handle =
                ControlChannelHandle::new((*config).clone(), self.config.remote_addr.clone());
            self.service_handles.insert(name.clone(), handle);
        }

        loop {
            tokio::select! {
                val = tokio::signal::ctrl_c() => {
                    match val {
                        Ok(()) => {}
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

struct RunDataChannelArgs {
    session_key: Nonce,
    remote_addr: String,
    local_addr: String,
}

async fn run_data_channel(args: Arc<RunDataChannelArgs>) -> Result<()> {
    // Retry at least every 100ms, at most for 10 seconds
    let backoff = ExponentialBackoff {
        max_interval: Duration::from_millis(100),
        max_elapsed_time: Some(Duration::from_secs(10)),
        ..Default::default()
    };

    // Connect to remote_addr
    let mut conn = backoff::future::retry(backoff, || async {
        Ok(TcpStream::connect(&args.remote_addr)
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
            let _ = io::copy_bidirectional(&mut conn, &mut local).await;
        }
    }
    Ok(())
}

struct ControlChannel {
    digest: ServiceDigest,
    service: ClientServiceConfig,
    shutdown_rx: oneshot::Receiver<u8>,
    remote_addr: String,
}

struct ControlChannelHandle {
    shutdown_tx: oneshot::Sender<u8>,
}

impl ControlChannel {
    #[instrument(skip(self), fields(service=%self.service.name))]
    async fn run(&mut self) -> Result<()> {
        let mut conn = TcpStream::connect(&self.remote_addr)
            .await
            .with_context(|| format!("Failed to connect to the server: {}", &self.remote_addr))?;

        // Send hello
        let hello_send =
            Hello::ControlChannelHello(CURRENT_PROTO_VRESION, self.digest[..].try_into().unwrap());
        conn.write_all(&bincode::serialize(&hello_send).unwrap())
            .await?;

        // Read hello
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
        match protocol::read_ack(&mut conn).await? {
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
        });

        loop {
            tokio::select! {
                val = protocol::read_control_cmd(&mut conn) => {
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
    fn new(service: ClientServiceConfig, remote_addr: String) -> ControlChannelHandle {
        let digest = protocol::digest(service.name.as_bytes());
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let mut s = ControlChannel {
            digest,
            service,
            shutdown_rx,
            remote_addr,
        };

        tokio::spawn(
            async move {
                loop {
                    if let Err(err) = s
                        .run()
                        .await
                        .with_context(|| "Failed to run the control channel")
                    {
                        let duration = Duration::from_secs(2);
                        error!("{:?}\n\nRetry in {:?}...", err, duration);
                        time::sleep(duration).await;
                    } else {
                        // Shutdown
                        break;
                    }
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

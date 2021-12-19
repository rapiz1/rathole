use anyhow::Result;
use clap::Parser;
use rathole::{run, Cli};
use tokio::{signal, sync::broadcast};

#[tokio::main]
async fn main() -> Result<()> {
    let args = Cli::parse();

    let (shutdown_tx, shutdown_rx) = broadcast::channel::<bool>(1);
    tokio::spawn(async move {
        if let Err(e) = signal::ctrl_c().await {
            // Something really weird happened. So just panic
            panic!("Failed to listen for the ctrl-c signal: {:?}", e);
        }

        if let Err(e) = shutdown_tx.send(true) {
            // shutdown signal must be catched and handle properly
            // `rx` must not be dropped
            panic!("Failed to send shutdown signal: {:?}", e);
        }
    });

    // TODO: use level from config
    tracing_subscriber::fmt::init();

    run(&args, shutdown_rx).await
}

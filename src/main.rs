use anyhow::Result;
use clap::Parser;
use rathole::{run, Cli};
use tokio;

#[tokio::main]
async fn main() -> Result<()> {
    let args = Cli::parse();
    run(&args).await
}

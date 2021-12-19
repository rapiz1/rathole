use anyhow::Result;
use common::{run_rathole_client, PING, PONG};
use rand::Rng;
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
    sync::broadcast,
    time,
};

use crate::common::run_rathole_server;

mod common;

const ECHO_SERVER_ADDR: &str = "localhost:8080";
const PINGPONG_SERVER_ADDR: &str = "localhost:8081";
const ECHO_SERVER_ADDR_EXPOSED: &str = "localhost:2334";
const PINGPONG_SERVER_ADDR_EXPOSED: &str = "localhost:2335";
const HITTER_NUM: usize = 4;

#[tokio::test]
async fn main() -> Result<()> {
    // Spawn a echo server
    tokio::spawn(async move {
        if let Err(e) = common::echo_server(ECHO_SERVER_ADDR).await {
            panic!("Failed to run the echo server for testing: {:?}", e);
        }
    });

    // Spawn a pingpong server
    tokio::spawn(async move {
        if let Err(e) = common::pingpong_server(PINGPONG_SERVER_ADDR).await {
            panic!("Failed to run the pingpong server for testing: {:?}", e);
        }
    });

    test("tests/tcp_transport.toml").await?;
    test("tests/tls_transport.toml").await?;

    Ok(())
}

async fn test(config_path: &'static str) -> Result<()> {
    let (client_shutdown_tx, client_shutdown_rx) = broadcast::channel(1);
    let (server_shutdown_tx, server_shutdown_rx) = broadcast::channel(1);

    // Start the client
    tokio::spawn(async move {
        run_rathole_client(&config_path, client_shutdown_rx)
            .await
            .unwrap();
    });

    // Sleep for 1 second. Expect the client keep retrying to reach the server
    time::sleep(Duration::from_secs(1)).await;

    // Start the server
    tokio::spawn(async move {
        run_rathole_server(&config_path, server_shutdown_rx)
            .await
            .unwrap();
    });
    time::sleep(Duration::from_secs(1)).await; // Wait for the client to retry

    echo_hitter(ECHO_SERVER_ADDR_EXPOSED).await.unwrap();
    pingpong_hitter(PINGPONG_SERVER_ADDR_EXPOSED).await.unwrap();

    // Simulate the client crash and restart
    client_shutdown_tx.send(true)?;
    time::sleep(Duration::from_millis(500)).await;
    let client_shutdown_rx = client_shutdown_tx.subscribe();
    tokio::spawn(async move {
        run_rathole_client(&config_path, client_shutdown_rx)
            .await
            .unwrap();
    });

    echo_hitter(ECHO_SERVER_ADDR_EXPOSED).await.unwrap();
    pingpong_hitter(PINGPONG_SERVER_ADDR_EXPOSED).await.unwrap();

    // Simulate the server crash and restart
    server_shutdown_tx.send(true)?;
    time::sleep(Duration::from_millis(500)).await;
    let server_shutdown_rx = server_shutdown_tx.subscribe();
    tokio::spawn(async move {
        run_rathole_server(&config_path, server_shutdown_rx)
            .await
            .unwrap();
    });
    time::sleep(Duration::from_secs(1)).await; // Wait for the client to retry

    // Simulate heavy load
    for _ in 0..HITTER_NUM / 2 {
        tokio::spawn(async move {
            echo_hitter(ECHO_SERVER_ADDR_EXPOSED).await.unwrap();
        });

        tokio::spawn(async move {
            pingpong_hitter(PINGPONG_SERVER_ADDR_EXPOSED).await.unwrap();
        });
    }

    // Shutdown
    server_shutdown_tx.send(true)?;
    client_shutdown_tx.send(true)?;

    Ok(())
}

async fn echo_hitter(addr: &str) -> Result<()> {
    let mut conn = TcpStream::connect(addr).await?;

    let mut wr = [0u8; 1024];
    let mut rd = [0u8; 1024];
    for _ in 0..100 {
        rand::thread_rng().fill(&mut wr);
        conn.write_all(&wr).await?;
        conn.read_exact(&mut rd).await?;
        assert_eq!(wr, rd);
    }

    Ok(())
}

async fn pingpong_hitter(addr: &str) -> Result<()> {
    let mut conn = TcpStream::connect(addr).await?;

    let wr = PING.as_bytes();
    let mut rd = [0u8; PONG.len()];

    for _ in 0..100 {
        conn.write_all(wr).await?;
        conn.read_exact(&mut rd).await?;
        assert_eq!(rd, PONG.as_bytes());
    }

    Ok(())
}

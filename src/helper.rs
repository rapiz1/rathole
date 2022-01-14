use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    net::SocketAddr,
    time::Duration,
};

use anyhow::{anyhow, Result};
use socket2::{SockRef, TcpKeepalive};
use tokio::net::{lookup_host, TcpStream, ToSocketAddrs, UdpSocket};
use tracing::trace;

// Tokio hesitates to expose this option...So we have to do it on our own :(
// The good news is that using socket2 it can be easily done, without losing portability.
// See https://github.com/tokio-rs/tokio/issues/3082
pub fn try_set_tcp_keepalive(
    conn: &TcpStream,
    keepalive_duration: Duration,
    keepalive_interval: Duration,
) -> Result<()> {
    let s = SockRef::from(conn);
    let keepalive = TcpKeepalive::new()
        .with_time(keepalive_duration)
        .with_interval(keepalive_interval);

    trace!(
        "Set TCP keepalive {:?} {:?}",
        keepalive_duration,
        keepalive_interval
    );

    Ok(s.set_tcp_keepalive(&keepalive)?)
}

#[allow(dead_code)]
pub fn feature_not_compile(feature: &str) -> ! {
    panic!(
        "The feature '{}' is not compiled in this binary. Please re-compile rathole",
        feature
    )
}

/// Create a UDP socket and connect to `addr`
pub async fn udp_connect<A: ToSocketAddrs>(addr: A) -> Result<UdpSocket> {
    let addr = lookup_host(addr)
        .await?
        .next()
        .ok_or(anyhow!("Failed to lookup the host"))?;

    let bind_addr = match addr {
        SocketAddr::V4(_) => "0.0.0.0:0",
        SocketAddr::V6(_) => ":::0",
    };

    let s = UdpSocket::bind(bind_addr).await?;
    s.connect(addr).await?;
    Ok(s)
}

// FIXME: These functions are for the load balance for UDP. But not used for now.
#[allow(dead_code)]
pub fn hash_socket_addr(a: &SocketAddr) -> u64 {
    let mut hasher = DefaultHasher::new();
    a.hash(&mut hasher);
    hasher.finish()
}

// Wait for the stabilization of https://doc.rust-lang.org/std/primitive.i64.html#method.log2
#[allow(dead_code)]
fn log2_floor(x: usize) -> u8 {
    (x as f64).log2().floor() as u8
}

#[allow(dead_code)]
pub fn floor_to_pow_of_2(x: usize) -> usize {
    if x == 1 {
        return 1;
    }
    let w = log2_floor(x);
    1 << w
}

#[cfg(test)]
mod test {
    use tokio::net::UdpSocket;

    use crate::helper::{floor_to_pow_of_2, log2_floor};

    use super::udp_connect;

    #[test]
    fn test_log2_floor() {
        let t = [
            (2, 1),
            (3, 1),
            (4, 2),
            (8, 3),
            (9, 3),
            (15, 3),
            (16, 4),
            (1023, 9),
            (1024, 10),
            (2000, 10),
            (2048, 11),
        ];
        for t in t {
            assert_eq!(log2_floor(t.0), t.1);
        }
    }

    #[test]
    fn test_floor_to_pow_of_2() {
        let t = [
            (1 as usize, 1 as usize),
            (2, 2),
            (3, 2),
            (4, 4),
            (5, 4),
            (15, 8),
            (31, 16),
            (33, 32),
            (1000, 512),
            (1500, 1024),
            (2300, 2048),
        ];
        for t in t {
            assert_eq!(floor_to_pow_of_2(t.0), t.1);
        }
    }

    #[tokio::test]
    async fn test_udp_connect() {
        let hello = "HELLO";

        let t = [("0.0.0.0:2333", "127.0.0.1:2333"), (":::2333", "::1:2333")];
        for t in t {
            let listener = UdpSocket::bind(t.0).await.unwrap();

            let handle = tokio::spawn(async move {
                let s = udp_connect(t.1).await.unwrap();
                s.send(hello.as_bytes()).await.unwrap();
                let mut buf = [0u8; 16];
                let n = s.recv(&mut buf).await.unwrap();
                assert_eq!(&buf[..n], hello.as_bytes());
            });

            let mut buf = [0u8; 16];
            let (n, addr) = listener.recv_from(&mut buf).await.unwrap();
            assert_eq!(&buf[..n], hello.as_bytes());
            listener.send_to(&buf[..n], addr).await.unwrap();

            handle.await.unwrap();
        }
    }
}

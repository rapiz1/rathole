use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    net::SocketAddr,
    time::Duration,
};

use anyhow::{Context, Result};
use socket2::{SockRef, TcpKeepalive};
use tokio::net::{TcpStream, ToSocketAddrs, UdpSocket};
use tracing::error;

// Tokio hesitates to expose this option...So we have to do it on our own :(
// The good news is that using socket2 it can be easily done, without losing portability.
// See https://github.com/tokio-rs/tokio/issues/3082
pub fn try_set_tcp_keepalive(conn: &TcpStream) -> Result<()> {
    let s = SockRef::from(conn);
    let keepalive = TcpKeepalive::new().with_time(Duration::from_secs(30));
    s.set_tcp_keepalive(&keepalive)
        .with_context(|| "Failed to set keepalive")
}

pub fn set_tcp_keepalive(conn: &TcpStream) {
    if let Err(e) = try_set_tcp_keepalive(conn) {
        error!(
            "Failed to set TCP keepalive. The connection maybe unstable: {:?}",
            e
        );
    }
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
    // FIXME: This only works for IPv4
    let s = UdpSocket::bind("0.0.0.0:0").await?;
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
    use crate::helper::{floor_to_pow_of_2, log2_floor};

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
}

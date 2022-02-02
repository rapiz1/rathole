use std::{net::SocketAddr, time::Duration};

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

/// Almost same as backoff::future::retry_notify
/// But directly expands to a loop
macro_rules! retry_notify {
    ($b: expr, $func: expr, $notify: expr) => {
        loop {
            match $func {
                Ok(v) => break Ok(v),
                Err(e) => match $b.next_backoff() {
                    Some(duration) => {
                        $notify(e, duration);
                        tokio::time::sleep(duration).await;
                    }
                    None => break Err(e),
                },
            }
        }
    };
}

pub(crate) use retry_notify;

#[cfg(test)]
mod test {
    use super::*;
    use backoff::{backoff::Backoff, ExponentialBackoff};
    #[tokio::test]
    async fn test_retry_notify() {
        let tests = [(3, Ok(())), (5, Err("try again"))];
        for (try_succ, expected) in tests {
            let mut b = ExponentialBackoff {
                current_interval: Duration::from_millis(100),
                initial_interval: Duration::from_millis(100),
                max_elapsed_time: Some(Duration::from_millis(210)),
                randomization_factor: 0.0,
                multiplier: 1.0,
                ..Default::default()
            };

            let mut notify_count = 0;
            let mut try_count = 0;
            let ret: Result<(), &str> = retry_notify!(
                b,
                {
                    try_count += 1;
                    if try_count == try_succ {
                        Ok(())
                    } else {
                        Err("try again")
                    }
                },
                |e, duration| {
                    notify_count += 1;
                    println!("{}: {}, {:?}", notify_count, e, duration);
                }
            );
            assert_eq!(ret, expected);
        }
    }
}

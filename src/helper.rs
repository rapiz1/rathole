use std::time::Duration;

use anyhow::{Context, Result};
use socket2::{SockRef, TcpKeepalive};
use tokio::net::TcpStream;

// Tokio hesitates to expose this option...So we have to do it on our own :(
// The good news is that using socket2 it can be easily done, without losing portablity.
// See https://github.com/tokio-rs/tokio/issues/3082
pub fn set_tcp_keepalive(conn: &TcpStream) -> Result<()> {
    let s = SockRef::from(conn);
    let keepalive = TcpKeepalive::new().with_time(Duration::from_secs(60));
    s.set_tcp_keepalive(&keepalive)
        .with_context(|| "Failed to set keepalive")
}

#[allow(dead_code)]
pub fn feature_not_compile(feature: &str) -> ! {
    panic!(
        "The feature '{}' is not compiled in this binary. Please re-compile rathole",
        feature
    )
}

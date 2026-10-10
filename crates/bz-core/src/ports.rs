//! Minimal local port helpers (full port-owner inspection lives in bz-net).

use std::net::{Ipv4Addr, SocketAddr, TcpListener};
use std::time::Duration;

/// Is something accepting TCP connections on `127.0.0.1:port`?
pub async fn is_listening(port: u16) -> bool {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    matches!(
        tokio::time::timeout(
            Duration::from_millis(300),
            tokio::net::TcpStream::connect(addr)
        )
        .await,
        Ok(Ok(_))
    )
}

/// Can we bind `0.0.0.0:port` / `127.0.0.1:port` right now?
pub fn can_bind(port: u16) -> bool {
    TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok()
}

/// Can we bind `0.0.0.0:port` (what Caddy does for `:80`/`:443`)?
pub fn can_bind_any(port: u16) -> bool {
    TcpListener::bind((Ipv4Addr::UNSPECIFIED, port)).is_ok()
}

/// Ask the OS for a free ephemeral port on 127.0.0.1.
pub fn free_port() -> std::io::Result<u16> {
    let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    Ok(l.local_addr()?.port())
}

/// First port in `start..start+span` that can be bound and is not in `taken`.
pub fn free_port_from(start: u16, span: u16, taken: &[u16]) -> Option<u16> {
    (start..start.saturating_add(span)).find(|p| !taken.contains(p) && can_bind(*p))
}

/// Wait until `port` accepts connections (or timeout). Returns success.
pub async fn wait_listening(port: u16, timeout: Duration) -> bool {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if is_listening(port).await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn detects_listener() {
        let l = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        assert!(is_listening(port).await);
        assert!(!can_bind(port));
        drop(l);
        let p = free_port().unwrap();
        assert!(p > 0);
        let f = free_port_from(p, 50, &[p]).unwrap();
        assert_ne!(f, p);
    }
}

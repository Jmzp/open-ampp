use std::net::TcpListener;
use std::time::Duration;

/// `true` if nothing listens on `127.0.0.1:port`.
pub fn is_free(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).is_ok()
}

/// `true` if something accepts connections on `127.0.0.1:port`.
pub async fn is_open(port: u16) -> bool {
    matches!(
        tokio::time::timeout(Duration::from_millis(300), tokio::net::TcpStream::connect(("127.0.0.1", port))).await,
        Ok(Ok(_))
    )
}

/// FastCGI port of a PHP version: `base + major*10 + minor` (8.3 → 9083).
pub fn php_fcgi_port(base: u16, php_version: &str) -> u16 {
    let mut parts = php_version.split('.').map(|p| p.parse::<u16>().unwrap_or(0));
    let (major, minor) = (parts.next().unwrap_or(0), parts.next().unwrap_or(0));
    base + major * 10 + minor
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn php_ports_are_distinct_per_minor() {
        assert_eq!(php_fcgi_port(9000, "8.3.12"), 9083);
        assert_eq!(php_fcgi_port(9000, "8.2.32"), 9082);
        assert_eq!(php_fcgi_port(9000, "7.4.33"), 9074);
    }

    #[tokio::test]
    async fn detects_open_and_free_ports() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(!is_free(port));
        assert!(is_open(port).await);
        drop(listener);
        assert!(is_free(port));
    }
}

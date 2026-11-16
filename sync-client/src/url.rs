/// Only https is allowed, except to this computer or when the person says the network is private.
pub fn check_url(url: &str, allow_private_http: bool) -> Result<(), String> {
    let lower = url.trim().to_lowercase();
    if lower.starts_with("https://") {
        return Ok(());
    }
    let Some(rest) = lower.strip_prefix("http://") else {
        return Err("The server address must start with https://".into());
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = if let Some(v6) = host.strip_prefix('[') { v6.split(']').next().unwrap_or("") } else { host.rsplit_once(':').map(|(h, p)| if p.chars().all(|c| c.is_ascii_digit()) { h } else { host }).unwrap_or(host) };
    // "localhost.evil.com" must not pass: compare the whole host name
    if host == "localhost" || host == "127.0.0.1" || host == "::1" || allow_private_http {
        return Ok(());
    }
    Err("Plain http:// is only allowed for this computer. Use https:// (or confirm the network is private).".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn check_url_https_ok() {
        assert!(check_url("https://sync.example.com", false).is_ok());
    }
    #[test]
    fn check_url_http_public_rejected() {
        assert!(check_url("http://sync.example.com", false).is_err());
        assert!(check_url("http://localhost.evil.com", false).is_err());
        assert!(check_url("ftp://x", false).is_err());
    }
    #[test]
    fn check_url_http_localhost_ok() {
        assert!(check_url("http://localhost:8787", false).is_ok());
        assert!(check_url("http://127.0.0.1:8787/x", false).is_ok());
        assert!(check_url("http://[::1]:8787", false).is_ok());
    }
    #[test]
    fn check_url_private_flag_allows_http() {
        assert!(check_url("http://100.64.0.5:8787", true).is_ok());
    }
}

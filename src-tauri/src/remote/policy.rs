#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalOrigin {
    raw: String,
    host: String,
}

impl CanonicalOrigin {
    pub fn parse(value: &str) -> Result<Self, String> {
        let trimmed = value.trim().trim_end_matches('/');
        let Some(rest) = trimmed.strip_prefix("https://") else {
            return Err("remote access requires an https origin".to_string());
        };
        if rest.is_empty() || rest.contains('/') || rest.contains('?') || rest.contains('#') {
            return Err("remote access origin must be scheme plus host only".to_string());
        }
        if rest.contains('@') || rest.chars().any(char::is_whitespace) {
            return Err("remote access origin host is invalid".to_string());
        }
        let host = rest.to_ascii_lowercase();
        Ok(Self {
            raw: format!("https://{host}"),
            host,
        })
    }

    pub fn raw(&self) -> &str {
        &self.raw
    }

    pub fn host(&self) -> &str {
        &self.host
    }

    pub fn allows_host_and_origin(&self, host: &str, origin: &str) -> bool {
        host.eq_ignore_ascii_case(&self.host) && origin == self.raw
    }
}

pub fn is_loopback_bind_host(host: &str) -> bool {
    matches!(host.trim(), "127.0.0.1" | "::1" | "localhost")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_origin_requires_https() {
        assert!(CanonicalOrigin::parse("https://wardian.tailnet.ts.net").is_ok());
        assert!(CanonicalOrigin::parse("http://wardian.tailnet.ts.net").is_err());
    }

    #[test]
    fn canonical_origin_rejects_path_query_and_fragment() {
        assert!(CanonicalOrigin::parse("https://wardian.tailnet.ts.net/path").is_err());
        assert!(CanonicalOrigin::parse("https://wardian.tailnet.ts.net?x=1").is_err());
        assert!(CanonicalOrigin::parse("https://wardian.tailnet.ts.net#frag").is_err());
    }

    #[test]
    fn origin_and_host_must_match_exactly() {
        let canonical = CanonicalOrigin::parse("https://wardian.tailnet.ts.net").unwrap();
        assert!(canonical
            .allows_host_and_origin("wardian.tailnet.ts.net", "https://wardian.tailnet.ts.net"));
        assert!(!canonical
            .allows_host_and_origin("other.tailnet.ts.net", "https://wardian.tailnet.ts.net"));
        assert!(!canonical
            .allows_host_and_origin("wardian.tailnet.ts.net", "https://other.tailnet.ts.net"));
    }

    #[test]
    fn canonical_origin_normalizes_host_case() {
        let canonical = CanonicalOrigin::parse("https://Wardian.Tailnet.TS.Net").unwrap();
        assert_eq!(canonical.raw(), "https://wardian.tailnet.ts.net");
        assert!(canonical
            .allows_host_and_origin("Wardian.Tailnet.TS.Net", "https://wardian.tailnet.ts.net"));
    }

    #[test]
    fn loopback_bind_policy_rejects_lan_and_wildcard_hosts() {
        assert!(is_loopback_bind_host("127.0.0.1"));
        assert!(is_loopback_bind_host("::1"));
        assert!(is_loopback_bind_host("localhost"));
        assert!(!is_loopback_bind_host("0.0.0.0"));
        assert!(!is_loopback_bind_host("192.168.1.10"));
    }
}

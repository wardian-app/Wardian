use std::collections::HashSet;

const MAX_APPROVED_CLIENT_ORIGINS: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalOrigin {
    raw: String,
    host: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonicalClientOrigin {
    raw: String,
}

impl CanonicalClientOrigin {
    pub fn raw(&self) -> &str {
        &self.raw
    }
}

pub fn canonicalized_client_origins(origins: &[String]) -> Result<Vec<String>, String> {
    if origins.len() > MAX_APPROVED_CLIENT_ORIGINS {
        return Err("too many approved client origins".to_string());
    }

    let mut canonical = Vec::with_capacity(origins.len());
    let mut seen = HashSet::with_capacity(origins.len());
    for value in origins {
        let origin = parse_client_origin(value)?;
        if !seen.insert(origin.raw().to_string()) {
            return Err("duplicate approved client origin".to_string());
        }
        canonical.push(origin.raw().to_string());
    }
    Ok(canonical)
}

pub fn is_approved_client_origin(approved_origins: &[String], request_origin: &str) -> bool {
    let Ok(origin) = parse_client_origin(request_origin) else {
        return false;
    };
    origin.raw() == request_origin
        && approved_origins
            .iter()
            .any(|approved| approved == request_origin)
}

pub fn parse_client_origin(value: &str) -> Result<CanonicalClientOrigin, String> {
    let value = value.trim();
    if value == "null" || value.contains('*') {
        return Err("client origin must identify one exact browser origin".to_string());
    }
    let url = url::Url::parse(value)
        .map_err(|_| "client origin must be a valid browser origin".to_string())?;
    let authority_contains_userinfo = value
        .split_once("://")
        .and_then(|(_, authority_and_path)| authority_and_path.split(['/', '?', '#']).next())
        .is_some_and(|authority| authority.contains('@'));
    if !url.username().is_empty()
        || url.password().is_some()
        || authority_contains_userinfo
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("client origin must be scheme plus authority only".to_string());
    }
    let scheme = url.scheme();
    let host = url
        .host()
        .ok_or_else(|| "client origin must include a host".to_string())?;
    let is_loopback = match host {
        url::Host::Ipv4(address) => address.is_loopback(),
        url::Host::Ipv6(address) => address.is_loopback(),
        url::Host::Domain(domain) => domain.eq_ignore_ascii_case("localhost"),
    };
    if scheme != "https" && !(scheme == "http" && is_loopback) {
        return Err("client origin must use HTTPS or configured HTTP loopback".to_string());
    }
    let authority_host = match host {
        url::Host::Ipv4(address) => address.to_string(),
        url::Host::Ipv6(address) => format!("[{address}]"),
        url::Host::Domain(domain) => domain.to_ascii_lowercase(),
    };
    let authority = match url.port() {
        Some(port) => format!("{authority_host}:{port}"),
        None => authority_host,
    };
    Ok(CanonicalClientOrigin {
        raw: format!("{scheme}://{authority}"),
    })
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
    fn client_origins_normalize_browser_origins_and_allow_only_configured_loopback_http() {
        let configured = canonicalized_client_origins(&[
            "https://Client.Example".to_string(),
            "https://client.example:8443".to_string(),
            "http://localhost:3000".to_string(),
            "http://127.0.0.1:80".to_string(),
        ])
        .expect("valid distinct origins");
        assert_eq!(
            configured,
            [
                "https://client.example",
                "https://client.example:8443",
                "http://localhost:3000",
                "http://127.0.0.1"
            ]
        );
        assert!(is_approved_client_origin(
            &configured,
            "https://client.example"
        ));
        assert!(!is_approved_client_origin(
            &configured,
            "https://client.example/"
        ));
        assert!(!is_approved_client_origin(
            &configured,
            "https://other.example"
        ));
        assert!(!is_approved_client_origin(&configured, "null"));
        assert!(!is_approved_client_origin(
            &configured,
            "http://127.0.0.1:80"
        ));

        for invalid in [
            "https://*",
            "https://*.example",
            "file://client.example",
            "http://client.example",
            "https://user@client.example",
            "https://@client.example",
            "https://client.example/path",
            "https://client.example?x=1",
            "https://client.example#frag",
        ] {
            assert!(parse_client_origin(invalid).is_err(), "accepted {invalid}");
        }
        assert!(parse_client_origin("https://client.example/").is_ok());
        assert!(canonicalized_client_origins(&[
            "https://client.example".to_string(),
            "https://CLIENT.example/".to_string(),
        ])
        .is_err());
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

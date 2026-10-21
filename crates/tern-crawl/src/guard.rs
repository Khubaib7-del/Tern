//! The network guard. See `docs/05-security-model.md`.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use url::{Host, Url};

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum GuardError {
    #[error("only http and https links can be read, not `{0}:`")]
    Scheme(String),
    #[error("links with a username or password are not read")]
    Credentials,
    #[error("the link has no host")]
    NoHost,
    #[error(
        "{host} points to a private or local address ({ip}), which Tern does not read from the web"
    )]
    Private { host: String, ip: IpAddr },
    #[error("could not look up {host}: {reason}")]
    Lookup { host: String, reason: String },
}

/// Checks the URL's form. Address checks happen in [`check_resolved`].
pub fn check_form(url: &Url) -> Result<(), GuardError> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(GuardError::Scheme(url.scheme().to_owned()));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(GuardError::Credentials);
    }
    if url.host().is_none() {
        return Err(GuardError::NoHost);
    }
    Ok(())
}

/// Checks form, then resolves the host and refuses non-public addresses.
pub async fn check_resolved(url: &Url, allow_private: bool) -> Result<(), GuardError> {
    check_form(url)?;
    if allow_private {
        return Ok(());
    }
    let host = url.host_str().unwrap_or_default().to_owned();
    let ips: Vec<IpAddr> = match url.host() {
        Some(Host::Ipv4(ip)) => vec![IpAddr::V4(ip)],
        Some(Host::Ipv6(ip)) => vec![IpAddr::V6(ip)],
        _ => {
            let port = url.port_or_known_default().unwrap_or(443);
            tokio::net::lookup_host((host.as_str(), port))
                .await
                .map_err(|e| GuardError::Lookup {
                    host: host.clone(),
                    reason: e.to_string(),
                })?
                .map(|a| a.ip())
                .collect()
        }
    };
    if ips.is_empty() {
        return Err(GuardError::Lookup {
            host,
            reason: "no addresses".into(),
        });
    }
    match ips.into_iter().find(|ip| !is_public(*ip)) {
        Some(ip) => Err(GuardError::Private { host, ip }),
        None => Ok(()),
    }
}

/// Whether an address is on the public internet.
pub fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_public_v4(v4),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_public_v4(v4);
            }
            let s = v6.segments();
            // NAT64 well-known prefix carries an IPv4 address.
            if s[0..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
                let o = v6.octets();
                return is_public_v4(Ipv4Addr::new(o[12], o[13], o[14], o[15]));
            }
            !(v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (s[0] & 0xfe00) == 0xfc00 // unique local
                || (s[0] & 0xffc0) == 0xfe80 // link local
                || (s[0] == 0x2001 && s[1] == 0x0db8) // documentation
                || v6 == Ipv6Addr::from([0u8; 16]))
        }
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_unspecified()
        || ip.is_multicast()
        || o[0] == 0
        || (o[0] == 100 && (o[1] & 0xc0) == 64) // shared address space
        || (o[0] == 192 && o[1] == 0 && o[2] == 0)
        || (o[0] == 198 && (o[1] & 0xfe) == 18) // benchmarking
        || o[0] >= 240)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().unwrap()
    }

    #[test]
    fn private_and_special_addresses_are_not_public() {
        for s in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "224.0.0.1",
            "255.255.255.255",
            "198.18.0.1",
            "::1",
            "::",
            "fe80::1",
            "fd00::1",
            "::ffff:127.0.0.1",
            "64:ff9b::a00:1",
            "2001:db8::1",
        ] {
            assert!(!is_public(ip(s)), "{s} should be blocked");
        }
        for s in [
            "1.1.1.1",
            "93.184.216.34",
            "2606:4700:4700::1111",
            "64:ff9b::101:101",
        ] {
            assert!(is_public(ip(s)), "{s} should be allowed");
        }
    }

    #[test]
    fn form_checks() {
        let bad = |s: &str| check_form(&Url::parse(s).unwrap()).unwrap_err();
        assert_eq!(bad("file:///etc/passwd"), GuardError::Scheme("file".into()));
        assert_eq!(bad("ftp://x.org/a"), GuardError::Scheme("ftp".into()));
        assert_eq!(bad("https://u:p@x.org/"), GuardError::Credentials);
        assert!(check_form(&Url::parse("https://arxiv.org/abs/1706.03762").unwrap()).is_ok());
    }

    #[tokio::test]
    async fn literal_private_hosts_are_refused_unless_allowed() {
        let u = Url::parse("http://127.0.0.1:8080/").unwrap();
        assert!(matches!(
            check_resolved(&u, false).await,
            Err(GuardError::Private { .. })
        ));
        assert!(check_resolved(&u, true).await.is_ok());
        let meta = Url::parse("http://169.254.169.254/latest/meta-data").unwrap();
        assert!(check_resolved(&meta, false).await.is_err());
        let localhost = Url::parse("http://localhost:9/").unwrap();
        assert!(check_resolved(&localhost, false).await.is_err());
    }
}

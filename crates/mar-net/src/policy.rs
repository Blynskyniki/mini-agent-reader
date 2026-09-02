//! What the client is allowed to reach.
//!
//! A rendering service fetches URLs chosen by whoever calls it, and then runs
//! scripts that choose more URLs. Without a policy that is a server-side
//! request forgery hole: a caller asks for `http://169.254.169.254/` and the
//! service reads cloud credentials for them. Blocking private address space by
//! default is the important part.

use std::net::IpAddr;
use url::{Host, Url};

#[derive(Debug, thiserror::Error)]
pub enum PolicyError {
    #[error("scheme {0:?} is not allowed")]
    Scheme(String),
    #[error("host {0:?} is not allowed")]
    Host(String),
    #[error("{0} is a private or reserved address")]
    PrivateAddress(IpAddr),
    #[error("URL has no host")]
    NoHost,
}

/// What a request is for. Affects headers, not permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    Document,
    Script,
    Xhr,
}

#[derive(Debug, Clone)]
pub struct Policy {
    /// Reject loopback, link-local, private and other reserved ranges.
    pub block_private_addresses: bool,
    /// When non-empty, only these hosts are reachable. Matching is on the
    /// registrable suffix, so "example.com" also allows "api.example.com".
    pub allow_hosts: Vec<String>,
    /// Hosts rejected outright, checked before the allow list.
    pub deny_hosts: Vec<String>,
}

impl Default for Policy {
    fn default() -> Self {
        Policy {
            // Safe by default. A caller running against a local test server
            // turns this off deliberately.
            block_private_addresses: true,
            allow_hosts: Vec::new(),
            deny_hosts: Vec::new(),
        }
    }
}

impl Policy {
    /// Allow private addresses, for local development and tests.
    pub fn permissive() -> Self {
        Policy {
            block_private_addresses: false,
            ..Policy::default()
        }
    }

    pub fn check(&self, url: &Url) -> Result<(), PolicyError> {
        match url.scheme() {
            "http" | "https" => {}
            other => return Err(PolicyError::Scheme(other.to_owned())),
        }

        let host = url.host().ok_or(PolicyError::NoHost)?;
        let host_str = url.host_str().unwrap_or_default().to_ascii_lowercase();

        if self.deny_hosts.iter().any(|d| matches_host(&host_str, d)) {
            return Err(PolicyError::Host(host_str));
        }
        if !self.allow_hosts.is_empty()
            && !self.allow_hosts.iter().any(|a| matches_host(&host_str, a))
        {
            return Err(PolicyError::Host(host_str));
        }

        if self.block_private_addresses {
            match host {
                Host::Ipv4(ip) => reject_if_private(IpAddr::V4(ip))?,
                Host::Ipv6(ip) => reject_if_private(IpAddr::V6(ip))?,
                Host::Domain(name) => {
                    // A literal written as a domain still has to be checked.
                    if let Ok(ip) = name.parse::<IpAddr>() {
                        reject_if_private(ip)?;
                    }
                    // "localhost" resolves to loopback on every normal host.
                    if name.eq_ignore_ascii_case("localhost")
                        || name.to_ascii_lowercase().ends_with(".localhost")
                    {
                        return Err(PolicyError::Host(name.to_owned()));
                    }
                }
            }
        }
        Ok(())
    }
}

/// Does `host` equal `pattern`, or is it a subdomain of it?
fn matches_host(host: &str, pattern: &str) -> bool {
    let pattern = pattern.trim_start_matches('.').to_ascii_lowercase();
    host == pattern || host.ends_with(&format!(".{pattern}"))
}

fn reject_if_private(ip: IpAddr) -> Result<(), PolicyError> {
    let blocked = match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_unspecified()
                // 100.64.0.0/10, carrier-grade NAT and common in cloud VPCs.
                || (v4.octets()[0] == 100 && (64..128).contains(&v4.octets()[1]))
                // 192.0.0.0/24 and 198.18.0.0/15, IETF reserved.
                || (v4.octets()[0] == 192 && v4.octets()[1] == 0 && v4.octets()[2] == 0)
                || (v4.octets()[0] == 198 && (18..20).contains(&v4.octets()[1]))
                // 240.0.0.0/4, reserved.
                || v4.octets()[0] >= 240
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                // fc00::/7 unique local, fe80::/10 link local.
                || (v6.segments()[0] & 0xfe00) == 0xfc00
                || (v6.segments()[0] & 0xffc0) == 0xfe80
                // An IPv4 address mapped into v6 must be judged as v4.
                || v6
                    .to_ipv4_mapped()
                    .is_some_and(|v4| reject_if_private(IpAddr::V4(v4)).is_err())
        }
    };
    if blocked {
        Err(PolicyError::PrivateAddress(ip))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(policy: &Policy, url: &str) -> Result<(), PolicyError> {
        policy.check(&Url::parse(url).unwrap())
    }

    #[test]
    fn private_and_reserved_space_is_blocked_by_default() {
        let p = Policy::default();
        for url in [
            "http://127.0.0.1/",
            "http://localhost:8080/",
            "http://10.0.0.5/",
            "http://192.168.1.1/",
            "http://172.16.0.1/",
            // The cloud metadata endpoint, the reason this check exists.
            "http://169.254.169.254/latest/meta-data/",
            "http://100.100.100.200/",
            "http://[::1]/",
            "http://[fd00::1]/",
            "http://[::ffff:127.0.0.1]/",
        ] {
            assert!(check(&p, url).is_err(), "should be blocked: {url}");
        }
        assert!(check(&p, "https://example.com/").is_ok());
        assert!(check(&p, "http://8.8.8.8/").is_ok());
    }

    #[test]
    fn non_http_schemes_are_refused() {
        let p = Policy::default();
        for url in [
            "file:///etc/passwd",
            "ftp://example.com/",
            "data:text/html,x",
        ] {
            assert!(check(&p, url).is_err(), "should be blocked: {url}");
        }
    }

    #[test]
    fn allow_and_deny_lists_cover_subdomains() {
        let p = Policy {
            allow_hosts: vec!["example.com".into()],
            deny_hosts: vec!["ads.example.com".into()],
            ..Policy::default()
        };
        assert!(check(&p, "https://example.com/a").is_ok());
        assert!(check(&p, "https://api.example.com/a").is_ok());
        assert!(check(&p, "https://ads.example.com/a").is_err(), "deny wins");
        assert!(check(&p, "https://other.com/a").is_err());
        // A suffix that is not a domain boundary must not match.
        assert!(check(&p, "https://notexample.com/a").is_err());
    }

    #[test]
    fn permissive_allows_localhost_for_tests() {
        assert!(check(&Policy::permissive(), "http://127.0.0.1:8080/").is_ok());
    }
}

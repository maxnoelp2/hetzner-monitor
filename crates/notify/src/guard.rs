//! SSRF guard for user-controlled targets (docs/design/security.md#outbound-requests-ssrf):
//! with private targets disallowed, every address a host resolves to must be public.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::Arc;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};

/// The error a blocked resolution carries (found again in `reqwest` error sources).
#[derive(Debug, thiserror::Error)]
#[error("target address not allowed")]
pub struct Blocked;

/// `true` for addresses a webhook must not reach when private targets are disallowed:
/// private, loopback, link-local (incl. cloud metadata `169.254.169.254`), CGNAT,
/// unspecified, broadcast, multicast, reserved, unique-local and IPv4-mapped forms of those.
pub fn is_blocked(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => blocked_v4(v4),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return blocked_v4(v4);
            }
            let seg = v6.segments();
            // NAT64 well-known prefix 64:ff9b::/96 embeds an IPv4 address.
            if seg[0] == 0x64 && seg[1] == 0xff9b && seg[2..6] == [0, 0, 0, 0] {
                let [a, b] = seg[6].to_be_bytes();
                let [c, d] = seg[7].to_be_bytes();
                return blocked_v4(Ipv4Addr::new(a, b, c, d));
            }
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || (seg[0] & 0xfe00) == 0xfc00 // unique local fc00::/7
                || (seg[0] & 0xffc0) == 0xfe80 // link-local fe80::/10
                || v6 == Ipv6Addr::from([0, 0, 0, 0, 0, 0, 0, 1])
        }
    }
}

fn blocked_v4(ip: Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_multicast()
        || a == 0
        || a >= 240
        || (a == 100 && (64..=127).contains(&b)) // CGNAT 100.64.0.0/10
}

/// DNS resolver that drops blocked addresses and fails if none is left. Applies to every
/// connection of the client, so DNS rebinding between check and connect cannot bypass it.
#[derive(Debug, Default)]
pub struct GuardedResolver;

impl Resolve for GuardedResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await?
                .filter(|a| !is_blocked(a.ip()))
                .collect();
            if addrs.is_empty() {
                return Err(Box::new(Blocked) as Box<dyn std::error::Error + Send + Sync>);
            }
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

pub fn resolver() -> Arc<GuardedResolver> {
    Arc::new(GuardedResolver)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_private_and_metadata_ranges() {
        for ip in [
            "10.0.0.1",
            "172.16.5.4",
            "192.168.1.1",
            "127.0.0.1",
            "169.254.169.254",
            "100.64.0.1",
            "0.0.0.0",
            "255.255.255.255",
            "224.0.0.1",
            "::1",
            "::",
            "fd00::1",
            "fe80::1",
            "::ffff:10.0.0.1",
            "64:ff9b::a9fe:a9fe",
        ] {
            assert!(is_blocked(ip.parse().unwrap()), "{ip} must be blocked");
        }
        for ip in ["1.1.1.1", "95.217.0.1", "100.128.0.1", "2a01:4f8::1"] {
            assert!(!is_blocked(ip.parse().unwrap()), "{ip} must be allowed");
        }
    }
}

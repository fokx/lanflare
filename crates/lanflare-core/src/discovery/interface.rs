//! Network interface discovery and filtering.

use std::net::{IpAddr, Ipv4Addr};

/// Restricts the network interfaces that are used.
///
/// Filters are matched against every address of an interface. A `*` matches one
/// or more characters that are not a `.`, so `192.168.1.*` matches
/// `192.168.1.42` but not `192.168.10.1`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct InterfaceFilter {
    /// When set, only interfaces with a matching address are used.
    pub whitelist: Option<Vec<String>>,

    /// When set, interfaces with a matching address are skipped.
    pub blacklist: Option<Vec<String>>,
}

impl InterfaceFilter {
    /// Creates a new empty filter that accepts all interfaces.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the address whitelist.
    pub fn with_whitelist(mut self, whitelist: Vec<String>) -> Self {
        self.whitelist = Some(whitelist);
        self
    }

    /// Sets the address blacklist.
    pub fn with_blacklist(mut self, blacklist: Vec<String>) -> Self {
        self.blacklist = Some(blacklist);
        self
    }

    /// Returns `true` when an interface with the given addresses must not be used.
    pub fn is_ignored(&self, addresses: &[IpAddr]) -> bool {
        if matches!(&self.whitelist, Some(wl) if !any_address_matches(addresses, wl)) {
            return true;
        }

        if matches!(&self.blacklist, Some(bl) if any_address_matches(addresses, bl)) {
            return true;
        }

        false
    }

}

/// Returns `true` when at least one of `addresses` matches at least one of `patterns`.
fn any_address_matches(addresses: &[IpAddr], patterns: &[String]) -> bool {
    addresses.iter().any(|address| {
        let address = address.to_string();
        patterns
            .iter()
            .any(|pattern| matches_ip_filter(pattern, &address))
    })
}

/// Matches the whole address against a filter pattern.
pub fn matches_ip_filter(pattern: &str, address: &str) -> bool {
    match pattern.split_once('*') {
        None => pattern == address,
        Some((prefix, rest)) => {
            let Some(remainder) = address.strip_prefix(prefix) else {
                return false;
            };

            // `*` must consume at least one character and may not cross a `.`.
            let max_len = remainder
                .find('.')
                .unwrap_or(remainder.len())
                .min(remainder.len());

            (1..=max_len).any(|len| matches_ip_filter(rest, &remainder[len..]))
        }
    }
}

/// A local IPv4 address that a multicast socket can be bound to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalInterfaceV4 {
    /// The name of the interface, only used for logging.
    pub name: String,

    /// The IPv4 address of the interface, used to join the multicast group.
    pub address: Ipv4Addr,
}

/// A local interface that an IPv6 multicast socket can be bound to.
///
/// IPv6 group membership is joined per interface rather than per address, so a
/// single socket per interface covers all of its IPv6 addresses.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LocalInterfaceV6 {
    /// The name of the interface, only used for logging.
    pub name: String,

    /// The index of the interface, used to join the multicast group.
    pub index: u32,
}

/// The local network interfaces that multicast sockets can be bound to.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LocalInterfaces {
    /// One entry per IPv4 address of an interface.
    pub v4: Vec<LocalInterfaceV4>,

    /// One entry per interface that has at least one IPv6 address.
    pub v6: Vec<LocalInterfaceV6>,
}

/// Returns the addresses of all non-loopback interfaces that pass `filter`.
pub fn local_interfaces(filter: &InterfaceFilter) -> std::io::Result<LocalInterfaces> {
    struct Entry {
        name: String,
        index: Option<u32>,
        addresses: Vec<IpAddr>,
    }

    // Filters apply to an interface as a whole, so all of its addresses have to
    // be known before any of them can be accepted.
    let mut by_name: Vec<Entry> = Vec::new();
    for interface in if_addrs::get_if_addrs()? {
        if interface.is_loopback() {
            continue;
        }

        match by_name
            .iter_mut()
            .find(|entry| entry.name == interface.name)
        {
            Some(entry) => {
                entry.index = entry.index.or(interface.index);
                entry.addresses.push(interface.ip());
            }
            None => by_name.push(Entry {
                name: interface.name.clone(),
                index: interface.index,
                addresses: vec![interface.ip()],
            }),
        }
    }

    let mut result = LocalInterfaces {
        v4: Vec::new(),
        v6: Vec::new(),
    };
    for entry in by_name {
        if filter.is_ignored(&entry.addresses) {
            tracing::debug!(
                "Ignoring network interface {} ({:?})",
                entry.name,
                entry.addresses
            );
            continue;
        }

        let mut has_v6 = false;
        for address in &entry.addresses {
            match address {
                // IPv4 group membership is joined per address.
                IpAddr::V4(address) => result.v4.push(LocalInterfaceV4 {
                    name: entry.name.clone(),
                    address: *address,
                }),
                IpAddr::V6(_) => has_v6 = true,
            }
        }

        if has_v6 {
            match entry.index {
                Some(index) => result.v6.push(LocalInterfaceV6 {
                    name: entry.name,
                    index,
                }),
                // Joining an IPv6 group requires the interface index.
                None => tracing::debug!(
                    "Interface {} has no index, not using it for IPv6 multicast",
                    entry.name
                ),
            }
        }
    }

    Ok(result)
}

/// Determines if an interface is an active LAN subnet suitable for a /24 sweep.
///
/// Filters out loopback, virtual containers (docker, veth, vboxnet), and point-to-point
/// VPN / CGNAT ranges (Tailscale `100.64.0.0/10`, wireguard, tun/tap) which do not carry /24 LANs.
pub fn is_sweepable_interface(name: &str, ip: &Ipv4Addr) -> bool {
    if ip.is_loopback() {
        return false;
    }

    // Skip CGNAT / Carrier Grade NAT (100.64.0.0/10) used by Tailscale, VPNs, etc.
    let octets = ip.octets();
    if octets[0] == 100 && (octets[1] & 0xC0) == 64 {
        return false;
    }

    let lower = name.to_ascii_lowercase();
    if lower.starts_with("tailscale")
        || lower.starts_with("docker")
        || lower.starts_with("vboxnet")
        || lower.starts_with("veth")
        || lower.starts_with("br-")
    {
        return false;
    }

    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    #[test]
    fn test_matches_ip_filter() {
        assert!(matches_ip_filter("192.168.1.*", "192.168.1.100"));
        assert!(matches_ip_filter("192.168.1.*", "192.168.1.1"));
        assert!(!matches_ip_filter("192.168.1.*", "192.168.2.1"));
        assert!(!matches_ip_filter("192.168.1.*", "192.168.10.1"));
        assert!(matches_ip_filter("10.0.*.*", "10.0.1.2"));
        assert!(matches_ip_filter("127.0.0.1", "127.0.0.1"));
        assert!(!matches_ip_filter("127.0.0.1", "127.0.0.2"));
    }

    #[test]
    fn test_interface_filter_whitelist_blacklist() {
        let ip = IpAddr::V4(Ipv4Addr::new(192, 168, 1, 50));
        let filter = InterfaceFilter::new().with_whitelist(vec!["192.168.1.*".to_string()]);
        assert!(!filter.is_ignored(&[ip]));

        let filter = InterfaceFilter::new().with_whitelist(vec!["10.0.0.*".to_string()]);
        assert!(filter.is_ignored(&[ip]));

        let filter = InterfaceFilter::new().with_blacklist(vec!["192.168.1.*".to_string()]);
        assert!(filter.is_ignored(&[ip]));
    }
}

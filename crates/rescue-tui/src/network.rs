// SPDX-License-Identifier: MIT OR Apache-2.0

//! Network primitives for the rescue-tui Network overlay (#655 Phase 1B).
//!
//! Pure helpers around `/sys/class/net` and the `ip` busybox applet.
//! No DHCP execution lives here — that's done by the worker thread in
//! `main.rs::spawn_dhcp_worker`. This module only owns:
//!
//! - [`enumerate_interfaces`] — list ethernet ifaces with link state
//! - [`read_lease`]            — read the post-DHCP IPv4 / gateway / DNS
//! - [`NetworkIface`] + [`NetworkLease`] — pure data structs
//!
//! Why a separate module? The state machine needs these as plain
//! `Vec<NetworkIface>` / `NetworkLease` so unit tests can populate
//! `Screen::Network` without touching the filesystem. `main.rs` calls
//! [`enumerate_interfaces`] only at overlay-open time.

use std::path::Path;

/// One ethernet interface visible to the rescue-tui at overlay-open time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkIface {
    /// Kernel name (`eth0`, `enp1s0`, `wlan0`, …).
    pub name: String,
    /// Up / down per `/sys/class/net/<n>/operstate`. Mirrors what
    /// `ip link` reports.
    pub link_state: LinkState,
    /// Current IPv4 address, if any. Read from `/proc/net/fib_trie`
    /// at enumerate time. `None` means "no address bound" — the
    /// usual pre-DHCP state.
    pub ipv4: Option<String>,
}

/// Coarse link state of a network interface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LinkState {
    /// Carrier present, ready for DHCP.
    Up,
    /// No carrier (cable unplugged, NIC disabled).
    Down,
    /// `/sys` lookup failed or returned an unrecognized value.
    Unknown,
}

impl LinkState {
    /// Short label for the table column.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            LinkState::Up => "up",
            LinkState::Down => "down",
            LinkState::Unknown => "?",
        }
    }
}

/// Lease state observed AFTER `udhcpc` returns. Built by `read_lease`
/// from `/proc/net/fib_trie` + `/etc/resolv.conf`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkLease {
    /// IPv4 address with prefix length (e.g. `192.168.1.42/24`).
    pub ipv4: String,
    /// Default-route gateway, if any (e.g. `192.168.1.1`).
    pub gateway: Option<String>,
    /// DNS resolvers from `/etc/resolv.conf` in declaration order.
    pub nameservers: Vec<String>,
}

/// Enumerate ethernet-style interfaces visible at this moment. Skips
/// `lo` and any interface whose `/sys/class/net/<n>/type` doesn't
/// match Ethernet (`type == 1`). On any read error, returns whatever
/// we managed to gather — best-effort, not an error result.
#[must_use]
pub fn enumerate_interfaces() -> Vec<NetworkIface> {
    enumerate_interfaces_under(Path::new("/sys/class/net"))
}

/// Test-shimmed variant of [`enumerate_interfaces`]. Lets tests point
/// at a fake `/sys/class/net` tree without monkey-patching `/`.
pub(crate) fn enumerate_interfaces_under(root: &Path) -> Vec<NetworkIface> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return out;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy().to_string();
        if name == "lo" {
            continue;
        }
        let dir = entry.path();
        if !is_ethernet(&dir) {
            continue;
        }
        let link_state = read_link_state(&dir);
        let ipv4 = read_ipv4_for(&name);
        out.push(NetworkIface {
            name,
            link_state,
            ipv4,
        });
    }
    // Stable order — kernel iface enumeration is unstable across boots,
    // and the operator's cursor position should be predictable.
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn is_ethernet(iface_dir: &Path) -> bool {
    // /sys/class/net/<n>/type == 1 means ARPHRD_ETHER. Wireless +
    // VLAN + bridge all report different values; we only enumerate
    // wired Ethernet for Phase 1B (Wi-Fi is Phase 4).
    let path = iface_dir.join("type");
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .is_some_and(|v| v == 1)
}

fn read_link_state(iface_dir: &Path) -> LinkState {
    let path = iface_dir.join("operstate");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return LinkState::Unknown;
    };
    match raw.trim() {
        "up" => LinkState::Up,
        "down" | "lowerlayerdown" | "notpresent" => LinkState::Down,
        _ => LinkState::Unknown,
    }
}

/// Read the first IPv4 address bound to `iface`, if any. Parses
/// `/proc/net/fib_trie` since rescue-tui shouldn't depend on `ip`
/// being on PATH at enumerate time (it lives at `/bin/ip` in the
/// initramfs but nowhere else for unit tests).
fn read_ipv4_for(iface: &str) -> Option<String> {
    // /proc/net/fib_trie carries the kernel's FIB; entries look like:
    //   |-- 192.168.1.0
    //      /24 universe UNICAST
    //      |-- 192.168.1.42
    //         /32 host LOCAL
    // The host-LOCAL entries are interface IPs. Without an extra
    // lookup we can't easily attribute them per-iface from /proc;
    // shell out to `ip -4 -o addr show dev <iface>` if it's on PATH.
    let output = std::process::Command::new("ip")
        .args(["-4", "-o", "addr", "show", "dev", iface])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_first_inet_line(&stdout)
}

/// Parse the first IPv4 address out of `ip -o addr show dev <n>` output.
///
/// One line, tokens like:
///   `1: eth0    inet 192.168.1.42/24 brd 192.168.1.255 ...`
///
/// Returns `192.168.1.42/24` (with the prefix) so the renderer can show
/// netmask context. Pure function — easy to unit test.
fn parse_first_inet_line(s: &str) -> Option<String> {
    for line in s.lines() {
        let mut toks = line.split_ascii_whitespace();
        // Skip ahead to "inet"; the next token is the address.
        while let Some(tok) = toks.next() {
            if tok == "inet"
                && let Some(addr) = toks.next()
            {
                return Some(addr.to_string());
            }
        }
    }
    None
}

/// Read the current lease state for `iface` after `udhcpc` has run.
/// Combines `ip -4 addr` (for IPv4 + prefix), `ip -4 route` (for the
/// default gateway), and `/etc/resolv.conf` (for nameservers).
///
/// # Errors
///
/// Returns `Err` only when no IPv4 address is bound on `iface` — that
/// state typically means `udhcpc` exited 0 but the upstream sent NAK
/// or the lease script failed to apply the address. Gateway and DNS
/// are best-effort and may legitimately be empty (e.g. a carrier LAN
/// with no upstream router); they don't trigger this error path.
pub fn read_lease(iface: &str) -> Result<NetworkLease, String> {
    let ipv4 = read_ipv4_for(iface).ok_or_else(|| format!("no IPv4 address on {iface}"))?;
    let gateway = read_default_gateway_for(iface);
    let nameservers = read_resolv_conf_nameservers();
    Ok(NetworkLease {
        ipv4,
        gateway,
        nameservers,
    })
}

fn read_default_gateway_for(iface: &str) -> Option<String> {
    let output = std::process::Command::new("ip")
        .args(["-4", "route", "show", "default", "dev", iface])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_default_via(&stdout)
}

/// Parse the IP after `via` in `ip -4 route show default dev eth0` output.
/// Pure — unit-testable.
fn parse_default_via(s: &str) -> Option<String> {
    for line in s.lines() {
        let mut toks = line.split_ascii_whitespace();
        while let Some(tok) = toks.next() {
            if tok == "via"
                && let Some(addr) = toks.next()
            {
                return Some(addr.to_string());
            }
        }
    }
    None
}

fn read_resolv_conf_nameservers() -> Vec<String> {
    let Ok(raw) = std::fs::read_to_string("/etc/resolv.conf") else {
        return Vec::new();
    };
    parse_resolv_conf(&raw)
}

/// Pull `nameserver <ip>` lines out of resolv.conf-shaped text.
/// Pure — unit-testable.
fn parse_resolv_conf(s: &str) -> Vec<String> {
    s.lines()
        .filter_map(|line| {
            line.trim()
                .strip_prefix("nameserver ")
                .map(|rest| rest.trim().to_string())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn parse_first_inet_line_returns_address_with_prefix() {
        let out = "1: eth0    inet 192.168.1.42/24 brd 192.168.1.255 scope global eth0\n";
        assert_eq!(parse_first_inet_line(out), Some("192.168.1.42/24".into()));
    }

    #[test]
    fn parse_first_inet_line_returns_none_for_link_only_iface() {
        // Interface up but no IPv4 bound — `ip` emits no `inet` line.
        let out = "1: eth0    inet6 fe80::1/64 scope link tentative\n";
        assert_eq!(parse_first_inet_line(out), None);
    }

    #[test]
    fn parse_first_inet_line_handles_multiple_addresses() {
        // Bonded iface or dual-stack — we report the first address.
        let out = "\
1: eth0    inet 10.0.0.5/24 brd 10.0.0.255 scope global eth0\n\
1: eth0    inet 192.168.99.1/24 brd 192.168.99.255 scope global secondary eth0\n";
        assert_eq!(parse_first_inet_line(out), Some("10.0.0.5/24".into()));
    }

    #[test]
    fn parse_default_via_finds_gateway_address() {
        let out = "default via 192.168.1.1 dev eth0 src 192.168.1.42 metric 100\n";
        assert_eq!(parse_default_via(out), Some("192.168.1.1".into()));
    }

    #[test]
    fn parse_default_via_none_when_no_via_token() {
        // Direct route, no gateway needed.
        let out = "default dev eth0 scope link\n";
        assert_eq!(parse_default_via(out), None);
    }

    #[test]
    fn parse_resolv_conf_returns_nameservers_in_order() {
        let raw = "\
# generated by udhcpc-script
search example.local
nameserver 8.8.8.8
nameserver 1.1.1.1
options edns0
";
        assert_eq!(parse_resolv_conf(raw), vec!["8.8.8.8", "1.1.1.1"]);
    }

    #[test]
    fn parse_resolv_conf_skips_search_and_options() {
        let raw = "search foo.local\noptions timeout:1\nnameserver 9.9.9.9\n";
        assert_eq!(parse_resolv_conf(raw), vec!["9.9.9.9"]);
    }

    #[test]
    fn parse_resolv_conf_handles_extra_whitespace() {
        let raw = "nameserver    1.0.0.1   \n";
        assert_eq!(parse_resolv_conf(raw), vec!["1.0.0.1"]);
    }

    #[test]
    fn enumerate_interfaces_under_skips_loopback_and_non_ethernet() {
        // Build a fake /sys/class/net under a properly-randomized
        // tempdir (tempfile crate; cleans up on drop). Avoids the
        // semgrep `temp-dir.temp-dir` flag on `std::env::temp_dir()`
        // — predictable filenames are TOCTOU-prone even in tests.
        let tmp_handle = tempfile::tempdir().unwrap_or_else(|e| panic!("tempdir: {e}"));
        let tmp = tmp_handle.path();

        // lo: type=772 (loopback) — must be skipped
        let lo = tmp.join("lo");
        std::fs::create_dir_all(&lo).unwrap();
        std::fs::write(lo.join("type"), "772\n").unwrap();
        std::fs::write(lo.join("operstate"), "unknown\n").unwrap();

        // wlan0: type=1 (ethernet from kernel POV) — included
        // (Yes — kernel reports type=1 for wireless too. Real Wi-Fi
        // discrimination is via wireless extensions; Phase 4. For
        // Phase 1B we treat anything type=1 as connectable.)
        let wlan0 = tmp.join("wlan0");
        std::fs::create_dir_all(&wlan0).unwrap();
        std::fs::write(wlan0.join("type"), "1\n").unwrap();
        std::fs::write(wlan0.join("operstate"), "down\n").unwrap();

        // eth0: type=1, up
        let eth0 = tmp.join("eth0");
        std::fs::create_dir_all(&eth0).unwrap();
        std::fs::write(eth0.join("type"), "1\n").unwrap();
        std::fs::write(eth0.join("operstate"), "up\n").unwrap();

        // bond0: type=24 — not ethernet, must be skipped
        let bond0 = tmp.join("bond0");
        std::fs::create_dir_all(&bond0).unwrap();
        std::fs::write(bond0.join("type"), "24\n").unwrap();
        std::fs::write(bond0.join("operstate"), "up\n").unwrap();

        let ifaces = enumerate_interfaces_under(tmp);
        let names: Vec<&str> = ifaces.iter().map(|i| i.name.as_str()).collect();
        assert_eq!(names, vec!["eth0", "wlan0"]);
        assert_eq!(ifaces[0].link_state, LinkState::Up);
        assert_eq!(ifaces[1].link_state, LinkState::Down);
    }
}

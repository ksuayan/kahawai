//! Tailscale discovery (P0 Phase 2, spec §2.1).
//!
//! At startup the server looks for a Tailscale interface: first
//! `tailscale ip -4`, falling back to scanning local interfaces for the
//! CGNAT range Tailscale uses (`100.64.0.0/10`). No interface name is
//! ever hardcoded. The MagicDNS name comes from `tailscale status
//! --json` on a best-effort basis.
//!
//! Nothing here is tested against a live tailnet in CI — there is none.
//! The parsing helpers are unit-tested against fixtures; the "Tailscale
//! only" bind refusal is tested by injecting [`TailscaleInfo`]/`None`.

use std::net::{IpAddr, Ipv4Addr};
use std::time::Duration;

/// What we know about the machine's Tailscale presence.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct TailscaleInfo {
    /// The tailnet IPv4, e.g. `100.96.12.34`.
    pub ipv4: Ipv4Addr,
    /// MagicDNS name where known, e.g. `kahawai-mac` (short form; the
    /// `--json` status reports the FQDN and we take the first label).
    pub magic_dns: Option<String>,
}

/// `100.64.0.0/10`: the CGNAT range Tailscale assigns tailnet addresses
/// from.
pub fn is_cgnat(ip: &Ipv4Addr) -> bool {
    let bits = u32::from(*ip);
    bits & 0xFFC0_0000 == 0x6440_0000
}

/// Run a command with a hard 3 s timeout on a helper thread, so a hung
/// `tailscale` binary can never hang server startup. `None` on timeout,
/// spawn failure, non-zero exit, or empty output.
fn run(cmd: &str, args: &[&str]) -> Option<String> {
    let cmd = cmd.to_string();
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let out = std::process::Command::new(&cmd).args(&args).output();
        let _ = tx.send(out);
    });
    let output = rx.recv_timeout(Duration::from_secs(3)).ok()?.ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    Some(text).filter(|t| !t.is_empty())
}

/// Parse `tailscale ip -4` output: the first IPv4-looking line wins.
fn parse_ip4(output: &str) -> Option<Ipv4Addr> {
    output
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .find_map(|l| l.parse::<Ipv4Addr>().ok())
}

/// Parse `tailscale status --json`: `.Self.DNSName`, first label of the
/// FQDN (`"kahawai-mac.tail123.ts.net."` → `"kahawai-mac"`).
fn parse_magic_dns(json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let fqdn = v.get("Self")?.get("DNSName")?.as_str()?;
    let short = fqdn.trim_end_matches('.').split('.').next()?;
    if short.is_empty() {
        return None;
    }
    Some(short.to_string())
}

/// Local interface addresses (unix). Implemented against `getifaddrs`
/// directly so no new dependency is needed for a startup probe.
#[cfg(unix)]
fn interface_addrs() -> Vec<IpAddr> {
    let mut out = Vec::new();
    // SAFETY: getifaddrs/freeifaddrs paired per the man page; every
    // pointer is null-checked before dereference.
    unsafe {
        let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut ifap) != 0 || ifap.is_null() {
            return out;
        }
        let mut cur = ifap;
        while !cur.is_null() {
            let ifa = &*cur;
            if !ifa.ifa_addr.is_null() && (*ifa.ifa_addr).sa_family as i32 == libc::AF_INET {
                let sin = &*(ifa.ifa_addr as *const libc::sockaddr_in);
                let ip = Ipv4Addr::from(u32::from_be(sin.sin_addr.s_addr));
                out.push(IpAddr::V4(ip));
            }
            cur = (*cur).ifa_next;
        }
        libc::freeifaddrs(ifap);
    }
    out
}

#[cfg(not(unix))]
fn interface_addrs() -> Vec<IpAddr> {
    Vec::new()
}

/// Best-effort Tailscale detection. `None` means "not detected" — the
/// caller says so plainly (spec §2.1) rather than guessing.
pub fn detect() -> Option<TailscaleInfo> {
    // 1. The CLI knows best.
    if let Some(output) = run("tailscale", &["ip", "-4"]) {
        if let Some(ipv4) = parse_ip4(&output) {
            let magic_dns = run("tailscale", &["status", "--json"])
                .as_deref()
                .and_then(parse_magic_dns);
            return Some(TailscaleInfo { ipv4, magic_dns });
        }
    }
    // 2. Fall back to scanning interfaces for the CGNAT range.
    let found = interface_addrs().into_iter().find_map(|a| match a {
        IpAddr::V4(v4) if is_cgnat(&v4) => Some(v4),
        _ => None,
    });
    found.map(|ipv4| TailscaleInfo {
        ipv4,
        magic_dns: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cgnat_range_check() {
        assert!(is_cgnat(&"100.64.0.1".parse().unwrap()));
        assert!(is_cgnat(&"100.127.255.254".parse().unwrap()));
        assert!(!is_cgnat(&"100.128.0.1".parse().unwrap()));
        assert!(!is_cgnat(&"192.168.1.10".parse().unwrap()));
        assert!(!is_cgnat(&"127.0.0.1".parse().unwrap()));
    }

    #[test]
    fn parses_tailscale_ip4_output() {
        assert_eq!(
            parse_ip4("100.96.12.34\n"),
            Some("100.96.12.34".parse().unwrap())
        );
        assert_eq!(parse_ip4(""), None);
        assert_eq!(parse_ip4("not an ip\n"), None);
    }

    #[test]
    fn parses_magic_dns_from_status_json() {
        let json = r#"{"Self": {"DNSName": "kahawai-mac.tail12345.ts.net."}}"#;
        assert_eq!(parse_magic_dns(json), Some("kahawai-mac".to_string()));
        assert_eq!(parse_magic_dns(r#"{"Self": {}}"#), None);
        assert_eq!(parse_magic_dns("not json"), None);
    }

    #[test]
    fn detect_does_not_panic_without_tailscale() {
        // No assertion on the outcome — CI has no tailnet — only that
        // probing a missing daemon returns promptly and safely.
        let _ = detect();
    }
}

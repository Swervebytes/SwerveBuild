//! Capability enforcement. Every outward-reaching operation a node can perform
//! funnels through this module with the workflow's `Permissions` in hand —
//! structural, like the shadow-mode arg builder in Automations. Hard caps here
//! (scheme, size, time, redirect and resolver checks, canonical path prefixes)
//! are not liftable from workflow JSON.

use crate::error::{ErrorKind, NodeError};
use crate::model::{FsPermission, NetworkPermission};
use serde_json::Value;
use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Response body cap (10 MB).
const MAX_RESPONSE_BYTES: u64 = 10 * 1024 * 1024;
const MAX_REDIRECTS: usize = 5;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Per-request total timeout default / ceiling.
pub const DEFAULT_HTTP_TIMEOUT_SECS: u64 = 30;
pub const MAX_HTTP_TIMEOUT_SECS: u64 = 120;

// ------------------------------------------------------------------ network

/// Is this host allowed by the list? Empty list = any host (the private-IP
/// policy still applies). Patterns: exact (case-insensitive) or `*.suffix`.
pub fn host_allowed(host: &str, allow: &[String]) -> bool {
    if allow.is_empty() {
        return true;
    }
    let host = host.to_ascii_lowercase();
    allow.iter().any(|pat| {
        let pat = pat.trim().to_ascii_lowercase();
        if let Some(suffix) = pat.strip_prefix("*.") {
            host.ends_with(&format!(".{suffix}")) || host == suffix
        } else {
            host == pat
        }
    })
}

/// Credential-bearing headers that must not survive a cross-origin redirect.
fn is_sensitive_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "authorization" | "cookie" | "proxy-authorization"
    )
}

/// Reject non-public addresses unless the workflow opted in.
fn ip_allowed(ip: &IpAddr, private_ok: bool) -> bool {
    if private_ok {
        return true;
    }
    match ip {
        IpAddr::V4(v4) => {
            !(v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                // CGNAT 100.64.0.0/10 (Tailscale et al.)
                || (v4.octets()[0] == 100 && (v4.octets()[1] & 0xC0) == 64))
        }
        IpAddr::V6(v6) => {
            // Any address that embeds an IPv4 gets the v4 policy applied to the
            // embedded address — otherwise a private target could be smuggled
            // through a transitional form.
            if let Some(mapped) = v6.to_ipv4_mapped() {
                return ip_allowed(&IpAddr::V4(mapped), private_ok);
            }
            let segs = v6.segments();
            let embedded = |hi: u16, lo: u16| {
                Ipv4Addr::new((hi >> 8) as u8, hi as u8, (lo >> 8) as u8, lo as u8)
            };
            // 6to4 2002::/16 (v4 in segs 1-2)
            if segs[0] == 0x2002 {
                return ip_allowed(&IpAddr::V4(embedded(segs[1], segs[2])), private_ok);
            }
            // NAT64 well-known 64:ff9b::/96 (v4 in the low 32 bits)
            if segs[0] == 0x0064 && segs[1] == 0xff9b {
                return ip_allowed(&IpAddr::V4(embedded(segs[6], segs[7])), private_ok);
            }
            // Deprecated IPv4-compatible ::a.b.c.d (upper 96 bits zero), excluding
            // :: and ::1 which the specials below already cover.
            if segs[..6].iter().all(|s| *s == 0) && !(segs[6] == 0 && segs[7] <= 1) {
                return ip_allowed(&IpAddr::V4(embedded(segs[6], segs[7])), private_ok);
            }
            let seg0 = segs[0];
            !(v6.is_loopback()
                || v6.is_unspecified()
                // ULA fc00::/7
                || (seg0 & 0xFE00) == 0xFC00
                // link-local fe80::/10
                || (seg0 & 0xFFC0) == 0xFE80)
        }
    }
}

/// Resolve a `host:port` netloc, enforce the IP policy, and PIN the result:
/// the returned addresses are exactly what the connection will use, closing
/// the DNS-rebinding gap between check and connect.
fn resolve_checked(netloc: &str, private_ok: bool) -> std::io::Result<Vec<SocketAddr>> {
    let addrs: Vec<SocketAddr> = netloc.to_socket_addrs()?.collect();
    let allowed: Vec<SocketAddr> = addrs
        .iter()
        .copied()
        .filter(|a| ip_allowed(&a.ip(), private_ok))
        .collect();
    if allowed.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("{netloc} resolves only to blocked (private/loopback) addresses; enable private_ips for this workflow to allow it"),
        ));
    }
    Ok(allowed)
}

#[derive(Debug, Clone)]
pub struct HttpRequestSpec {
    pub method: String,
    pub url: String,
    /// (name, value) pairs.
    pub headers: Vec<(String, String)>,
    pub query: Vec<(String, String)>,
    /// "none" | "json" | "text" | "form"
    pub body_type: String,
    pub body: Value,
    pub timeout_secs: u64,
}

#[derive(Debug, Clone)]
pub struct HttpResponseData {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    /// JSON-parsed when the content type says JSON, else a string.
    pub body: Value,
    pub url: String,
}

/// Permission-checked HTTP. Redirects are followed manually so every hop is
/// re-validated against the host allowlist and the IP policy.
pub fn http_request(policy: &NetworkPermission, spec: &HttpRequestSpec) -> Result<HttpResponseData, NodeError> {
    if !policy.enabled {
        return Err(NodeError::permission(
            "this workflow has no network permission; enable it in Permissions",
        ));
    }
    let timeout = spec.timeout_secs.clamp(1, MAX_HTTP_TIMEOUT_SECS);
    let mut method = spec.method.to_ascii_uppercase();
    if !matches!(method.as_str(), "GET" | "POST" | "PUT" | "PATCH" | "DELETE" | "HEAD") {
        return Err(NodeError::params(format!("unsupported HTTP method {method}")));
    }
    let mut current = url::Url::parse(&spec.url).map_err(|e| NodeError::params(format!("bad url {}: {e}", spec.url)))?;
    // Query params are appended once, to the initial URL.
    for (k, v) in &spec.query {
        current.query_pairs_mut().append_pair(k, v);
    }
    let mut send_body = true;
    // Working copy so cross-origin redirects can drop credential headers.
    let mut headers = spec.headers.clone();

    for _hop in 0..=MAX_REDIRECTS {
        if current.scheme() != "http" && current.scheme() != "https" {
            return Err(NodeError::permission(format!("scheme {} is not allowed", current.scheme())));
        }
        let host = current
            .host_str()
            .ok_or_else(|| NodeError::params("url has no host"))?
            .to_string();
        if !host_allowed(&host, &policy.hosts) {
            return Err(NodeError::permission(format!(
                "host {host} is not in this workflow's allowed hosts"
            )));
        }
        // Literal-IP targets get checked here; named hosts get checked (and
        // pinned) inside the resolver at connect time.
        if let Ok(ip) = host.trim_matches(['[', ']']).parse::<IpAddr>() {
            if !ip_allowed(&ip, policy.private_ips) {
                return Err(NodeError::permission(format!(
                    "address {ip} is private; enable private_ips for this workflow to allow it"
                )));
            }
        }

        let private_ok = policy.private_ips;
        let agent = ureq::AgentBuilder::new()
            .redirects(0)
            .timeout_connect(CONNECT_TIMEOUT)
            .timeout(Duration::from_secs(timeout))
            .resolver(move |netloc: &str| resolve_checked(netloc, private_ok))
            .user_agent("SwerveWorkflows/0.1")
            .build();

        let mut req = agent.request(&method, current.as_str());
        for (name, value) in &headers {
            req = req.set(name, value);
        }

        let result = if send_body {
            match spec.body_type.as_str() {
                "json" => req.send_json(spec.body.clone()),
                "text" => req.send_string(spec.body.as_str().unwrap_or_default()),
                "form" => {
                    let pairs: Vec<(String, String)> = spec
                        .body
                        .as_object()
                        .map(|o| {
                            o.iter()
                                .map(|(k, v)| {
                                    let val = match v {
                                        Value::String(s) => s.clone(),
                                        other => other.to_string(),
                                    };
                                    (k.clone(), val)
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    let borrowed: Vec<(&str, &str)> =
                        pairs.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
                    req.send_form(&borrowed)
                }
                _ => req.call(),
            }
        } else {
            req.call()
        };

        let response = match result {
            Ok(resp) => resp,
            // Non-2xx is a response, not a transport failure — the workflow
            // decides what a 404 means.
            Err(ureq::Error::Status(_code, resp)) => resp,
            Err(ureq::Error::Transport(t)) => {
                return Err(NodeError::new(ErrorKind::Http, format!("request failed: {t}")));
            }
        };

        let status = response.status();
        if matches!(status, 301 | 302 | 303 | 307 | 308) {
            if let Some(location) = response.header("Location") {
                let next = current
                    .join(location)
                    .map_err(|e| NodeError::new(ErrorKind::Http, format!("bad redirect target: {e}")))?;
                // Crossing to a different origin drops credential headers so a
                // Bearer token / Cookie isn't replayed to the redirect target
                // (the leak curl and reqwest defend against).
                if next.origin() != current.origin() {
                    headers.retain(|(name, _)| !is_sensitive_header(name));
                }
                // 303 always becomes GET; 301/302 drop the body like browsers do.
                if status == 303 || ((status == 301 || status == 302) && method != "GET" && method != "HEAD") {
                    method = "GET".to_string();
                    send_body = false;
                } else if status == 307 || status == 308 {
                    // method + body preserved
                } else {
                    send_body = false;
                }
                current = next;
                continue;
            }
        }

        // Terminal response: read capped body.
        let headers: Vec<(String, String)> = response
            .headers_names()
            .iter()
            .filter_map(|n| response.header(n).map(|v| (n.to_ascii_lowercase(), v.to_string())))
            .collect();
        let content_type = response.content_type().to_ascii_lowercase();
        let mut buf: Vec<u8> = Vec::new();
        response
            .into_reader()
            .take(MAX_RESPONSE_BYTES + 1)
            .read_to_end(&mut buf)
            .map_err(|e| NodeError::new(ErrorKind::Http, format!("reading response: {e}")))?;
        if buf.len() as u64 > MAX_RESPONSE_BYTES {
            return Err(NodeError::new(
                ErrorKind::Http,
                format!("response exceeded the {} MB cap", MAX_RESPONSE_BYTES / (1024 * 1024)),
            ));
        }
        let text = String::from_utf8_lossy(&buf).to_string();
        let body = if content_type.contains("json") {
            serde_json::from_str(&text).unwrap_or(Value::String(text))
        } else {
            Value::String(text)
        };
        return Ok(HttpResponseData { status, headers, body, url: current.to_string() });
    }

    Err(NodeError::new(ErrorKind::Http, format!("more than {MAX_REDIRECTS} redirects")))
}

// ------------------------------------------------------------------ filesystem

/// Resolve `path` against an allowlist of directory prefixes. Both sides are
/// canonicalized, which resolves junctions and symlinks (the A4 lesson), so a
/// link inside a granted directory cannot escape it. For writes the file may
/// not exist yet — its PARENT must exist and sit inside a granted prefix.
fn check_path(path: &str, allow: &[String], for_write: bool) -> Result<PathBuf, NodeError> {
    if allow.is_empty() {
        let verb = if for_write { "write" } else { "read" };
        return Err(NodeError::permission(format!(
            "this workflow has no file {verb} permission; grant a folder in Permissions"
        )));
    }
    let target = Path::new(path);
    let canonical = if for_write {
        let parent = target
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .ok_or_else(|| NodeError::params(format!("path {path} has no parent directory")))?;
        let file_name = target
            .file_name()
            .ok_or_else(|| NodeError::params(format!("path {path} has no file name")))?;
        let parent = parent
            .canonicalize()
            .map_err(|e| NodeError::new(ErrorKind::Fs, format!("parent folder of {path}: {e}")))?;
        let candidate = parent.join(file_name);
        // If the target already exists, canonicalize it fully so a symlink at the
        // final component is resolved to its real target and THAT is what the
        // containment check sees — otherwise a planted link could escape the
        // granted folder on write. New files (no symlink to follow) use the
        // parent-relative path.
        if candidate.symlink_metadata().is_ok() {
            candidate
                .canonicalize()
                .map_err(|e| NodeError::new(ErrorKind::Fs, format!("{path}: {e}")))?
        } else {
            candidate
        }
    } else {
        target
            .canonicalize()
            .map_err(|e| NodeError::new(ErrorKind::Fs, format!("{path}: {e}")))?
    };
    let inside = allow.iter().any(|prefix| {
        Path::new(prefix)
            .canonicalize()
            .map(|granted| canonical.starts_with(&granted))
            .unwrap_or(false)
    });
    if !inside {
        let verb = if for_write { "writable" } else { "readable" };
        return Err(NodeError::permission(format!(
            "{path} is outside this workflow's {verb} folders"
        )));
    }
    Ok(canonical)
}

pub fn fs_read(policy: &FsPermission, path: &str) -> Result<Vec<u8>, NodeError> {
    let canonical = check_path(path, &policy.read, false)?;
    std::fs::read(&canonical).map_err(|e| NodeError::new(ErrorKind::Fs, format!("reading {path}: {e}")))
}

pub fn fs_write(policy: &FsPermission, path: &str, bytes: &[u8], append: bool) -> Result<(), NodeError> {
    let canonical = check_path(path, &policy.write, true)?;
    let result = if append {
        use std::fs::OpenOptions;
        use std::io::Write;
        OpenOptions::new()
            .create(true)
            .append(true)
            .open(&canonical)
            .and_then(|mut f| f.write_all(bytes))
    } else {
        std::fs::write(&canonical, bytes)
    };
    result.map_err(|e| NodeError::new(ErrorKind::Fs, format!("writing {path}: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_patterns() {
        let allow = vec!["api.github.com".to_string(), "*.roaringbytes.com".to_string()];
        assert!(host_allowed("api.github.com", &allow));
        assert!(host_allowed("API.GITHUB.COM", &allow));
        assert!(host_allowed("feedback.roaringbytes.com", &allow));
        assert!(host_allowed("roaringbytes.com", &allow)); // bare apex matches its wildcard
        assert!(!host_allowed("github.com", &allow));
        assert!(!host_allowed("evilroaringbytes.com", &allow));
        assert!(host_allowed("anything.example", &[])); // empty list = any host
    }

    #[test]
    fn private_ips_blocked_by_default() {
        for bad in ["127.0.0.1", "10.0.0.5", "192.168.0.22", "172.16.9.1", "169.254.1.1", "100.64.0.1", "0.0.0.0"] {
            let ip: IpAddr = bad.parse().unwrap();
            assert!(!ip_allowed(&ip, false), "{bad} must be blocked");
            assert!(ip_allowed(&ip, true), "{bad} must pass with the opt-in");
        }
        let v6_bad: IpAddr = "fd12::1".parse().unwrap();
        assert!(!ip_allowed(&v6_bad, false));
        let mapped: IpAddr = "::ffff:192.168.0.1".parse().unwrap();
        assert!(!ip_allowed(&mapped, false), "v4-mapped v6 must be unwrapped and blocked");
        let ok: IpAddr = "93.184.216.34".parse().unwrap();
        assert!(ip_allowed(&ok, false));
    }

    #[test]
    fn ipv6_embedded_private_v4_is_blocked() {
        // Transitional forms that embed a private/loopback IPv4 must be caught:
        // 6to4 (10.0.0.1), NAT64 (192.168.0.1), deprecated ::a.b.c.d (127.0.0.1).
        for bad in ["2002:0a00:0001::", "64:ff9b::c0a8:1", "::7f00:1"] {
            let ip: IpAddr = bad.parse().unwrap();
            assert!(!ip_allowed(&ip, false), "{bad} must be blocked");
            assert!(ip_allowed(&ip, true), "{bad} must pass with the opt-in");
        }
        // A public v4 embedded in 6to4 (93.184.216.34) stays allowed.
        let ok: IpAddr = "2002:5db8:d822::".parse().unwrap();
        assert!(ip_allowed(&ok, false));
    }

    #[cfg(windows)]
    #[test]
    fn fs_write_rejects_symlink_escaping_the_grant() {
        use std::os::windows::fs::symlink_file;
        let base = std::env::temp_dir().join(format!("swf-symlink-{}", std::process::id()));
        let grant = base.join("grant");
        std::fs::create_dir_all(&grant).unwrap();
        let secret = base.join("secret.txt");
        std::fs::write(&secret, b"secret").unwrap();
        let link = grant.join("out.txt");
        // Needs Developer Mode / admin; if the OS refuses the symlink, skip.
        if symlink_file(&secret, &link).is_err() {
            let _ = std::fs::remove_dir_all(&base);
            return;
        }
        let policy = FsPermission { read: vec![], write: vec![grant.to_string_lossy().to_string()] };
        let out = fs_write(&policy, link.to_str().unwrap(), b"evil", false);
        assert!(out.is_err(), "a write through a symlink out of the grant must be refused");
        assert_eq!(std::fs::read(&secret).unwrap(), b"secret", "the escape target must be untouched");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn network_disabled_refuses() {
        let policy = NetworkPermission::default();
        let spec = HttpRequestSpec {
            method: "GET".into(),
            url: "https://example.com".into(),
            headers: vec![],
            query: vec![],
            body_type: "none".into(),
            body: Value::Null,
            timeout_secs: 5,
        };
        let err = http_request(&policy, &spec).unwrap_err();
        assert_eq!(err.kind, ErrorKind::Permission);
    }

    #[test]
    fn fs_requires_grants_and_containment() {
        let dir = std::env::temp_dir().join(format!("swf-perm-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("inner")).unwrap();
        std::fs::write(dir.join("inner").join("a.txt"), b"hi").unwrap();

        let none = FsPermission::default();
        assert_eq!(
            fs_read(&none, dir.join("inner").join("a.txt").to_str().unwrap()).unwrap_err().kind,
            ErrorKind::Permission
        );

        let granted = FsPermission {
            read: vec![dir.join("inner").to_string_lossy().to_string()],
            write: vec![dir.join("inner").to_string_lossy().to_string()],
        };
        assert_eq!(fs_read(&granted, dir.join("inner").join("a.txt").to_str().unwrap()).unwrap(), b"hi");

        // Traversal out of the grant is refused even though the file exists.
        std::fs::write(dir.join("outside.txt"), b"no").unwrap();
        let sneaky = dir.join("inner").join("..").join("outside.txt");
        assert_eq!(fs_read(&granted, sneaky.to_str().unwrap()).unwrap_err().kind, ErrorKind::Permission);

        // Writes create files inside the grant; parent must already exist.
        fs_write(&granted, dir.join("inner").join("new.txt").to_str().unwrap(), b"x", false).unwrap();
        assert_eq!(std::fs::read(dir.join("inner").join("new.txt")).unwrap(), b"x");
        let out = fs_write(&granted, dir.join("elsewhere").join("new.txt").to_str().unwrap(), b"x", false);
        assert!(out.is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }
}

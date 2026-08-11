//! OS keystore seam — Windows Credential Manager (S36 / Step 8 prereq).
//!
//! VISION standing guardrail: **stream keys never sit in plaintext JSON.**
//! `providers.json` holds the custom-endpoint API key and local server token in
//! the clear today (deferred audit item A3); this module is the replacement
//! seam, introduced before the first secret that actually warrants it.
//!
//! # Why the webview can never read a secret back
//!
//! Anything registered as a `#[tauri::command]` is callable by **any** JS in the
//! main webview. S21b showed how far that reaches: an attacker with a foothold
//! there can invoke commands at will. So the Tauri surface here is deliberately
//! **write-and-forget** — set, delete, and "does one exist?" — and there is no
//! command that returns a secret value. Only Rust-side consumers (the future
//! RTMP sink) call [`get`]. A stolen stream key is a live-broadcast takeover,
//! which `live-safety.md` classes as irreversible.

use serde::{Deserialize, Serialize};

/// Credential Manager "service" all our entries live under.
const SERVICE: &str = "SwerveBuild";

/// Namespace for stream/broadcast credentials (Step 8).
pub const STREAM_NAMESPACE: &str = "stream";

/// Whether a secret exists, without revealing it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretStatus {
    pub name: String,
    pub exists: bool,
}

/// Reject names that could collide, traverse, or produce unreadable entries.
///
/// Keys are user-facing in Credential Manager, so keep them boring and
/// predictable: `stream.twitch`, `stream.youtube`.
fn validate_name(name: &str) -> Result<&str, String> {
    let n = name.trim();
    if n.is_empty() {
        return Err("secret name is empty".into());
    }
    if n.len() > 128 {
        return Err("secret name too long (max 128)".into());
    }
    if !n
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
    {
        return Err("secret name may only contain letters, digits, '.', '-', '_'".into());
    }
    Ok(n)
}

fn entry(name: &str) -> Result<keyring::Entry, String> {
    let n = validate_name(name)?;
    keyring::Entry::new(SERVICE, n).map_err(|e| format!("keystore open failed: {e}"))
}

/// Store (or replace) a secret. Empty values are rejected — use [`delete`].
pub fn set(name: &str, value: &str) -> Result<(), String> {
    if value.is_empty() {
        return Err("secret value is empty (use delete to remove)".into());
    }
    entry(name)?
        .set_password(value)
        .map_err(|e| format!("keystore write failed: {e}"))
}

/// Read a secret. **Rust-internal only** — never expose this via a Tauri
/// command (see module docs). `Ok(None)` means "not set", which is distinct
/// from a keystore failure.
pub fn get(name: &str) -> Result<Option<String>, String> {
    match entry(name)?.get_password() {
        Ok(v) => Ok(Some(v)),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(e) => Err(format!("keystore read failed: {e}")),
    }
}

/// Remove a secret. Deleting one that does not exist is not an error, so the
/// UI's "clear" button is idempotent.
pub fn delete(name: &str) -> Result<(), String> {
    match entry(name)?.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()),
        Err(e) => Err(format!("keystore delete failed: {e}")),
    }
}

/// Does a secret exist? Never reveals the value.
pub fn status(name: &str) -> Result<SecretStatus, String> {
    let exists = get(name)?.is_some();
    Ok(SecretStatus {
        name: validate_name(name)?.to_string(),
        exists,
    })
}

/// Namespaced key for a stream destination, e.g. `stream.twitch`.
pub fn stream_key_name(target: &str) -> Result<String, String> {
    let t = validate_name(target)?;
    Ok(format!("{STREAM_NAMESPACE}.{t}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_must_be_boring_and_safe() {
        assert!(validate_name("stream.twitch").is_ok());
        assert!(validate_name("a-b_c.1").is_ok());
        assert!(validate_name("  padded  ").is_ok(), "trims");
        assert!(validate_name("").is_err());
        assert!(validate_name("   ").is_err());
        assert!(validate_name("has space").is_err());
        assert!(validate_name("path/traversal").is_err());
        assert!(validate_name("back\\slash").is_err());
        assert!(validate_name("emoji🔑").is_err());
        assert!(validate_name(&"x".repeat(129)).is_err());
    }

    #[test]
    fn empty_value_is_rejected_not_stored() {
        // An empty password would look like "set" while meaning nothing.
        assert!(set("stream.test-empty", "").is_err());
    }

    #[test]
    fn stream_names_are_namespaced() {
        assert_eq!(stream_key_name("twitch").unwrap(), "stream.twitch");
        assert!(stream_key_name("bad name").is_err());
    }

    /// Proves the secret really lands in **Windows Credential Manager** (the
    /// OS keystore the guardrail requires) rather than some fallback store,
    /// by looking for the entry in `cmdkey /list`.
    /// `cargo test -p swerve-build --lib secrets::tests::live_lands_in_credential_manager -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_lands_in_credential_manager() {
        let name = "stream.swerve-credmgr-check";
        let _ = delete(name);
        set(name, "value-should-not-be-visible").expect("set");

        let out = std::process::Command::new("cmdkey")
            .arg("/list")
            .output()
            .expect("run cmdkey");
        let listing = String::from_utf8_lossy(&out.stdout);
        let found = listing.contains(SERVICE) && listing.contains(name);
        eprintln!("cmdkey sees the entry: {found}");
        // The value itself must never appear in a listing.
        assert!(
            !listing.contains("value-should-not-be-visible"),
            "cmdkey listing must not expose the secret value"
        );
        let _ = delete(name);
        assert!(found, "entry not found in Windows Credential Manager");
    }

    /// The actual guardrail: a stored secret must appear in **no** file under
    /// the app data dir (`providers.json`, `data.json`, logs, anything).
    /// `cargo test -p swerve-build --lib secrets::tests::live_secret_never_hits_disk -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_secret_never_hits_disk() {
        // Distinctive enough that a substring hit can only be our value.
        let marker = "SWERVE-PLAINTEXT-CANARY-9f3a2b71";
        let name = "stream.swerve-disk-check";
        let _ = delete(name);
        set(name, marker).expect("set");

        fn scan(dir: &std::path::Path, marker: &str, hits: &mut Vec<String>) {
            let Ok(entries) = std::fs::read_dir(dir) else {
                return;
            };
            for e in entries.flatten() {
                let p = e.path();
                if p.is_dir() {
                    scan(&p, marker, hits);
                } else if let Ok(bytes) = std::fs::read(&p) {
                    if String::from_utf8_lossy(&bytes).contains(marker) {
                        hits.push(p.display().to_string());
                    }
                }
            }
        }

        let mut hits = Vec::new();
        scan(&crate::paths::data_dir(), marker, &mut hits);
        let _ = delete(name);

        eprintln!("scanned {}", crate::paths::data_dir().display());
        assert!(
            hits.is_empty(),
            "secret leaked to plaintext file(s): {hits:?}"
        );
        eprintln!("no plaintext leak — secret exists only in the keystore");
    }

    /// Live round-trip against the real Credential Manager. Ignored by default
    /// so CI (and any machine without a keystore) stays green.
    /// `cargo test -p swerve-build --lib secrets::tests::live_roundtrip -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn live_roundtrip() {
        let name = "stream.swerve-selftest";
        let _ = delete(name);
        assert!(!status(name).unwrap().exists, "should start absent");

        set(name, "super-secret-value").expect("set");
        assert!(status(name).unwrap().exists, "should exist after set");
        assert_eq!(get(name).unwrap().as_deref(), Some("super-secret-value"));

        // Replacing must overwrite, not append/duplicate.
        set(name, "rotated").expect("rotate");
        assert_eq!(get(name).unwrap().as_deref(), Some("rotated"));

        delete(name).expect("delete");
        assert!(!status(name).unwrap().exists, "should be gone");
        assert_eq!(get(name).unwrap(), None, "absent reads as None, not error");
        // Idempotent delete.
        delete(name).expect("second delete is a no-op");
        eprintln!("keystore round-trip OK");
    }
}

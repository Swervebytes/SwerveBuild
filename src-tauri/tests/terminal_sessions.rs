//! S11 live integration: stand up the real terminal control server in-process
//! and drive the full proxy → loopback socket → SessionManager → PowerShell path.
//! Proves persistent-session state survives across execs (the acceptance), plus
//! exit codes, paging, token/grant gating, and cleanup. Windows-only (PowerShell).

#![cfg(windows)]

use std::sync::Arc;
use swerve_build_lib::terminal;

/// Kills a session on drop so an early assertion failure doesn't orphan a shell.
struct SessionGuard(String);
impl Drop for SessionGuard {
    fn drop(&mut self) {
        let _ = terminal::session_close(&self.0);
    }
}

#[test]
fn persistent_session_survives_execs_and_persists_state() {
    // Grant on for the duration; restore afterward so the dev machine stays clean.
    let prev = terminal::load_grant();
    terminal::set_granted(true).expect("grant on");

    // Real control server: binds loopback, publishes term_control.json.
    let mgr = Arc::new(terminal::SessionManager::default());
    terminal::serve(mgr.clone()).expect("serve control server");

    // --- start a persistent session (defaults to the open project) ---
    let start = terminal::session_start(None, None).expect("start proxied");
    assert_eq!(start["ok"], true, "start not ok: {start}");
    let sid = start["sessionId"].as_str().expect("sessionId").to_string();
    let _guard = SessionGuard(sid.clone());

    // --- exec 1: set a variable AND change directory ---
    let e1 = terminal::session_exec(&sid, "$marker = 41; Set-Location src-tauri", Some(30)).expect("exec1");
    assert_eq!(e1["ok"], true, "exec1 not ok: {e1}");

    // --- exec 2: BOTH the variable and the cwd must have survived ---
    let e2 = terminal::session_exec(&sid, "\"$($marker + 1) @ $((Get-Location).Path)\"", Some(30)).expect("exec2");
    let out2 = e2["output"].as_str().unwrap_or("");
    assert!(out2.contains("42"), "variable did not persist across execs: {e2}");
    assert!(out2.to_lowercase().contains("src-tauri"), "cwd did not persist across execs: {e2}");

    // --- exit code propagation (native non-zero) ---
    let e3 = terminal::session_exec(&sid, "cmd /c exit 7", Some(30)).expect("exec3");
    assert_eq!(e3["exitCode"].as_i64(), Some(7), "exit code not propagated: {e3}");
    assert_eq!(e3["ok"], false, "non-zero exit should be ok:false: {e3}");

    // --- read paging returns the cumulative buffer ---
    let r = terminal::session_read(&sid, Some(0)).expect("read");
    assert_eq!(r["ok"], true, "read not ok: {r}");
    assert!(r["nextOffset"].as_u64().unwrap_or(0) > 0, "buffer should have grown: {r}");

    // --- list shows the live session ---
    let list = terminal::session_list().expect("list");
    let found = list["sessions"].as_array().map(|a| a.iter().any(|s| s["sessionId"] == sid.as_str())).unwrap_or(false);
    assert!(found, "session missing from list: {list}");

    // --- close kills it; a later exec must fail cleanly (a clear response, not a hang) ---
    let c = terminal::session_close(&sid).expect("close");
    assert_eq!(c["ok"], true, "close not ok: {c}");
    let after = terminal::session_exec(&sid, "1", Some(5)).expect("call still returns a response");
    assert_eq!(after["ok"], false, "exec after close should be ok:false: {after}");
    assert!(
        after["error"].as_str().unwrap_or("").contains("no such session"),
        "expected a clear 'no such session' error: {after}"
    );

    // restore grant
    let _ = terminal::set_granted(prev.granted);
}

// Note: the "no control server → clear error, no hang" path is covered by the
// sidecar smoke (scripts) rather than a second integration test here, because it
// depends on the ABSENCE of a published control file — which this test creates,
// making the two race under parallel test threads.

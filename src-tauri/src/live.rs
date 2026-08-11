//! Go-live approval tier (S38 / Step 8 prereq).
//!
//! Broadcast is the one irreversible action in this app. A bad file write is
//! recoverable; ten seconds of the wrong window on a live stream is not. So
//! going live gets its own permission tier, stricter than the terminal / app_ui
//! / browser grants — and unlike those, **an agent can never satisfy it**.
//!
//! # Why the existing grant pattern is not enough
//!
//! `terminal.rs` and `app_ui.rs` gate on "did the human tick a box in Settings".
//! That is the right shape for *repeatable, recoverable* actions: tick once,
//! the agent works. Going live must be decided **per stream, by a human who is
//! physically at the keyboard right now**, because the agent surface can reach
//! a plain Tauri command three different ways:
//!
//! 1. **MCP tools** — `bin/swervebuild_mcp.rs` calls library functions directly.
//! 2. **CDP drive** — `app_ui_click` / `app_ui_press` synthesize input into the
//!    webview, which can press any button the human can press.
//! 3. **Injected JS in the main webview** (S21b) — invokes any
//!    `#[tauri::command]` at will.
//!
//! So the gate is two independent layers, and a caller must clear **both**:
//!
//! - **Layer 1 — provenance ([`Caller`]).** Every entry point names who is
//!   asking. Only [`Caller::Human`] may arm. This is what structurally keeps
//!   our *own* agent surfaces out: the MCP binary, `jobs.rs` automations, and
//!   workflow nodes pass an agent caller and are refused before any probing
//!   happens. Layer 1 alone is a claim, not proof — paths 2 and 3 above run
//!   inside the human's own webview and would pass it.
//! - **Layer 2 — physical presence ([`Presence`]).** Proof the *operating
//!   system* saw a real human at this machine, which the webview cannot fake:
//!   remote debugging must be off for this process, real OS input must be
//!   recent, and the confirm modifier keys must be physically held at the
//!   moment of confirmation. CDP's `Input.dispatch*` never reaches the OS input
//!   queue, so a synthesized click leaves every one of these signals untouched.
//!
//! Both layers are pure functions ([`arm_gate`]) so every branch is unit-tested
//! without a keyboard, a webview, or a data dir.
//!
//! # Direction of failure
//!
//! Everything above guards **starting**. Stopping is the opposite: [`stop`] and
//! [`panic_cut`] take any caller, need no grant, no proof, and no arming — they
//! cannot fail and cannot be refused. An agent that notices something wrong on
//! the stream is *encouraged* to cut it. Fail-safe means failing toward off-air.
//!
//! # Not persisted, on purpose
//!
//! Live state lives in memory only. A crashed or restarted app is never live
//! and never armed — recovering "we were mid-stream" would mean going live
//! without a human in the loop, which is exactly what this module exists to
//! prevent. Only the *grant* (may this machine stream at all) is on disk.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const GRANT_FILE: &str = "live_grant.json";
const LOG_FILE: &str = "live_events.jsonl";

/// Audit lines retained (newest kept).
const LOG_KEEP: usize = 500;

/// How long a confirmation challenge stays valid. Short: it exists to bind the
/// human's keypress to *this* request, not to be a session.
const CHALLENGE_TTL: Duration = Duration::from_secs(30);

/// How long an armed state survives before it must be re-confirmed. Arming is
/// permission to start a stream in the next moment, not a standing permission.
const ARMED_TTL: Duration = Duration::from_secs(120);

/// Real OS input must have happened within this window of a confirmation.
/// Generous enough for a human who clicked and then reached for the keyboard,
/// tight enough that an idle unattended machine cannot be driven live.
const INPUT_FRESH_WINDOW: Duration = Duration::from_secs(10);

// ------------------------------------------------------------------ provenance

/// Who is asking. Constructed at each entry point; never taken from the caller's
/// own payload, so an agent cannot label itself `Human` by passing a string.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Caller {
    /// The desktop UI, driven by a person. A *claim* — see module docs; layer 2
    /// is what actually tests it.
    Human,
    /// An ACP agent acting through any in-app path.
    Agent,
    /// The MCP tool surface (`bin/swervebuild_mcp.rs`).
    Mcp,
    /// A scheduled / triggered automation (`jobs.rs`).
    Automation,
    /// A workflow graph node (`swerve-workflows`).
    Workflow,
}

impl Caller {
    /// Everything that is not a person at the keyboard.
    pub fn is_agent(self) -> bool {
        !matches!(self, Caller::Human)
    }

    pub fn label(self) -> &'static str {
        match self {
            Caller::Human => "human",
            Caller::Agent => "agent",
            Caller::Mcp => "mcp",
            Caller::Automation => "automation",
            Caller::Workflow => "workflow",
        }
    }
}

// ----------------------------------------------------------------------- grant

/// "May this machine broadcast at all." The coarse, persistent half of the
/// tier — off by default, same shape as the terminal / app_ui grants.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveGrant {
    pub granted: bool,
    #[serde(default)]
    pub updated_at: String,
}

impl Default for LiveGrant {
    fn default() -> Self {
        Self {
            granted: false,
            updated_at: String::new(),
        }
    }
}

fn grant_path() -> PathBuf {
    crate::paths::data_dir().join(GRANT_FILE)
}

pub fn load_grant() -> LiveGrant {
    let path = grant_path();
    if !path.is_file() {
        return LiveGrant::default();
    }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_default()
}

pub fn is_granted() -> bool {
    load_grant().granted
}

/// Enable / disable broadcasting for this machine. Only a human may *enable*;
/// revoking is allowed from anywhere (fail-safe direction), and revoking while
/// live cuts the stream.
pub fn set_granted(caller: Caller, granted: bool) -> Result<LiveGrant, String> {
    if granted && caller.is_agent() {
        return Err(format!(
            "{} may not enable broadcasting. Only a human can, in Settings → Live broadcast.",
            caller.label()
        ));
    }
    let grant = LiveGrant {
        granted,
        updated_at: crate::store::Store::now(),
    };
    let path = grant_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let raw = serde_json::to_string_pretty(&grant).map_err(|e| e.to_string())?;
    crate::paths::write_atomic(&path, raw.as_bytes()).map_err(|e| e.to_string())?;
    audit(caller, "grant_set", json!({ "granted": granted }));
    if !granted {
        // Revoking permission must not leave a stream running.
        stop(caller, "grant revoked");
    }
    Ok(grant)
}

// ------------------------------------------------------------------- presence

/// The layer-2 signals, gathered from the OS. Split from the gate so the
/// decision logic is testable without a real keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Presence {
    /// WebView2 remote debugging is open on this process. While true, anything
    /// on the machine can synthesize UI input, so going live is impossible.
    pub cdp_enabled: bool,
    /// The OS saw real keyboard/mouse input within [`INPUT_FRESH_WINDOW`].
    pub physical_input_fresh: bool,
    /// Both confirm modifiers are physically held right now.
    pub confirm_keys_down: bool,
}

/// Virtual-key codes for the confirm chord. Ctrl+Shift is deliberately awkward
/// to hold by accident and trivial to describe in the UI.
#[cfg(windows)]
const VK_SHIFT: i32 = 0x10;
#[cfg(windows)]
const VK_CONTROL: i32 = 0x11;

#[cfg(windows)]
#[repr(C)]
struct LastInputInfo {
    cb_size: u32,
    dw_time: u32,
}

#[cfg(windows)]
extern "system" {
    fn GetLastInputInfo(plii: *mut LastInputInfo) -> i32;
    fn GetAsyncKeyState(v_key: i32) -> i16;
    fn GetTickCount() -> u32;
}

/// Human-readable name of the confirm chord, for UI and error messages.
pub const CONFIRM_CHORD: &str = "Ctrl+Shift";

/// Ask the OS what it knows about the human at this machine.
///
/// `GetLastInputInfo` reports the last input the *system input queue* saw.
/// CDP's `Input.dispatchMouseEvent` / `dispatchKeyEvent` inject at the browser
/// event level and never touch that queue, so a driven click cannot refresh it.
#[cfg(windows)]
pub fn probe_presence() -> Presence {
    let physical_input_fresh = unsafe {
        let mut lii = LastInputInfo {
            cb_size: std::mem::size_of::<LastInputInfo>() as u32,
            dw_time: 0,
        };
        if GetLastInputInfo(&mut lii) == 0 {
            false
        } else {
            // Both are GetTickCount ms; wrapping_sub survives the 49.7-day wrap.
            let idle_ms = GetTickCount().wrapping_sub(lii.dw_time) as u64;
            idle_ms <= INPUT_FRESH_WINDOW.as_millis() as u64
        }
    };
    // High bit set = key is down at this instant.
    let confirm_keys_down = unsafe {
        (GetAsyncKeyState(VK_CONTROL) as u16 & 0x8000) != 0
            && (GetAsyncKeyState(VK_SHIFT) as u16 & 0x8000) != 0
    };
    Presence {
        cdp_enabled: crate::app_ui::cdp_should_enable(),
        physical_input_fresh,
        confirm_keys_down,
    }
}

/// Non-Windows builds cannot prove presence, so they never arm. The app ships
/// Windows-only (see `Cargo.toml`); this keeps `cargo check` honest elsewhere
/// rather than silently granting.
#[cfg(not(windows))]
pub fn probe_presence() -> Presence {
    Presence {
        cdp_enabled: crate::app_ui::cdp_should_enable(),
        physical_input_fresh: false,
        confirm_keys_down: false,
    }
}

// ------------------------------------------------------------------ the gate

/// The whole arming decision, as one pure function.
///
/// Order matters for the error message the human reads: provenance first (a
/// flat "agents may never do this"), then the cheap persistent grant, then the
/// physical checks that tell them what to *do* about it.
pub fn arm_gate(caller: Caller, granted: bool, p: &Presence) -> Result<(), String> {
    if caller.is_agent() {
        return Err(format!(
            "going live is a human-only action; {} may not arm it. (Stopping is always allowed.)",
            caller.label()
        ));
    }
    if !granted {
        return Err(
            "broadcasting is not enabled. A human must turn on \"Allow this machine to broadcast\" in Settings → Live broadcast."
                .into(),
        );
    }
    if p.cdp_enabled {
        return Err(
            "cannot go live while agent UI control (remote debugging) is enabled for this process — synthesized input could start a stream. Turn off Agent UI control and browser agent grant in Settings, then fully restart SwerveBuild."
                .into(),
        );
    }
    if !p.physical_input_fresh {
        return Err(format!(
            "no recent physical input at this machine (within {}s). Going live requires a person at the keyboard.",
            INPUT_FRESH_WINDOW.as_secs()
        ));
    }
    if !p.confirm_keys_down {
        return Err(format!(
            "hold {CONFIRM_CHORD} while confirming to go live."
        ));
    }
    Ok(())
}

// ----------------------------------------------------------------- live state

/// What the app is doing right now, broadcast-wise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Phase {
    /// Not live, not armed.
    Off,
    /// A human confirmed within [`ARMED_TTL`]; a stream may be started.
    Armed,
    /// On air.
    Live,
}

struct State {
    phase: Phase,
    /// Outstanding challenge: (nonce, issued_at). Single use.
    challenge: Option<(String, Instant)>,
    armed_at: Option<Instant>,
    live_since: Option<Instant>,
    /// Rule 2/3: per-stream opt-in for agent-driven scene / persona changes and
    /// preview→program promotion. In memory only — it dies with every stop, so
    /// it can never be inherited by the next stream.
    agent_scene_control: bool,
    /// Panic cut engaged (privacy scene showing).
    privacy_cut: bool,
}

impl State {
    const fn new() -> Self {
        Self {
            phase: Phase::Off,
            challenge: None,
            armed_at: None,
            live_since: None,
            agent_scene_control: false,
            privacy_cut: false,
        }
    }

    /// Expire `Armed` in place. Called before every read/decision so a stale arm
    /// can never be consumed.
    fn expire(&mut self) {
        if self.phase == Phase::Armed {
            let stale = self
                .armed_at
                .map(|t| t.elapsed() > ARMED_TTL)
                .unwrap_or(true);
            if stale {
                self.phase = Phase::Off;
                self.armed_at = None;
            }
        }
        if let Some((_, issued)) = &self.challenge {
            if issued.elapsed() > CHALLENGE_TTL {
                self.challenge = None;
            }
        }
    }
}

static STATE: Mutex<State> = Mutex::new(State::new());

/// Public snapshot. Deliberately says nothing secret — safe for any surface,
/// including agents, which *should* be able to see that a stream is running.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LiveStatus {
    pub phase: Phase,
    pub granted: bool,
    pub live: bool,
    pub agent_scene_control: bool,
    pub privacy_cut: bool,
    pub live_seconds: u64,
    /// Why arming would fail right now (`None` when it would succeed). Lets the
    /// UI show the blocking reason before the human commits.
    pub blocked_reason: Option<String>,
    pub confirm_chord: String,
}

pub fn status() -> LiveStatus {
    let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
    st.expire();
    let granted = is_granted();
    // Report the blocker without the chord requirement: the human has not been
    // asked to hold it yet, so surfacing it here would read as a broken state.
    let mut probe = probe_presence();
    probe.confirm_keys_down = true;
    let blocked_reason = arm_gate(Caller::Human, granted, &probe).err();
    LiveStatus {
        phase: st.phase,
        granted,
        live: st.phase == Phase::Live,
        agent_scene_control: st.agent_scene_control,
        privacy_cut: st.privacy_cut,
        live_seconds: st.live_since.map(|t| t.elapsed().as_secs()).unwrap_or(0),
        blocked_reason,
        confirm_chord: CONFIRM_CHORD.to_string(),
    }
}

/// Is a stream on air? The seam S39's RTMP sink checks before writing a frame.
pub fn is_live() -> bool {
    let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
    st.expire();
    st.phase == Phase::Live
}

// ------------------------------------------------------------------- arm flow

/// Step 1: ask for a challenge. Refuses early (and audibly) for agent callers so
/// a misrouted automation fails here rather than at a confusing keypress check.
pub fn request_arm(caller: Caller) -> Result<String, String> {
    // Everything except the chord, which the human has not pressed yet.
    let mut probe = probe_presence();
    probe.confirm_keys_down = true;
    arm_gate(caller, is_granted(), &probe)?;

    let nonce = uuid::Uuid::new_v4().to_string();
    let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
    st.expire();
    st.challenge = Some((nonce.clone(), Instant::now()));
    audit(caller, "arm_requested", json!({}));
    Ok(nonce)
}

/// Step 2: confirm with the challenge while holding the chord. Consumes the
/// nonce whether or not the checks pass, so a leaked nonce is never replayable.
pub fn confirm_arm(caller: Caller, nonce: &str) -> Result<LiveStatus, String> {
    let presence = probe_presence();
    let granted = is_granted();

    {
        let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
        st.expire();
        let taken = st.challenge.take();
        let Some((expected, _)) = taken else {
            return Err("no active go-live request, or it expired. Start again.".into());
        };
        // Constant-time-ish is overkill here (the nonce is not a secret against
        // an attacker who can already read the webview) but single-use matters.
        if expected != nonce {
            audit(caller, "arm_denied", json!({ "reason": "bad nonce" }));
            return Err("go-live request does not match. Start again.".into());
        }
    }

    if let Err(e) = arm_gate(caller, granted, &presence) {
        audit(caller, "arm_denied", json!({ "reason": e }));
        return Err(e);
    }

    let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
    st.phase = Phase::Armed;
    st.armed_at = Some(Instant::now());
    drop(st);
    audit(caller, "armed", json!({}));
    Ok(status())
}

/// Step 3: go on air. Re-runs the full gate — arming is not a bypass, it is a
/// second factor. S39's RTMP sink starts only after this returns `Ok`.
pub fn go_live(caller: Caller) -> Result<LiveStatus, String> {
    let presence = probe_presence();
    let granted = is_granted();
    {
        let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
        st.expire();
        if st.phase == Phase::Live {
            return Ok(status());
        }
        if st.phase != Phase::Armed {
            return Err(format!(
                "not armed. A human must confirm (hold {CONFIRM_CHORD}) within {}s of going live.",
                ARMED_TTL.as_secs()
            ));
        }
    }
    if let Err(e) = arm_gate(caller, granted, &presence) {
        audit(caller, "go_live_denied", json!({ "reason": e }));
        return Err(e);
    }

    let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
    st.phase = Phase::Live;
    st.live_since = Some(Instant::now());
    st.armed_at = None;
    st.privacy_cut = false;
    drop(st);
    audit(caller, "went_live", json!({}));
    Ok(status())
}

// -------------------------------------------------------------- stop / panic

/// Stop broadcasting. **Always allowed, from any caller, no grant, no proof,
/// cannot fail.** Clears the per-stream agent opt-in so the next stream starts
/// from denied.
pub fn stop(caller: Caller, reason: &str) -> LiveStatus {
    let was_live = {
        let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
        let was = st.phase == Phase::Live;
        st.phase = Phase::Off;
        st.challenge = None;
        st.armed_at = None;
        st.live_since = None;
        st.agent_scene_control = false;
        st.privacy_cut = false;
        was
    };
    if was_live {
        audit(caller, "stopped", json!({ "reason": reason }));
    }
    status()
}

/// Panic button: cut to the privacy scene immediately. Local, instant, never
/// routed through an agent path — and like [`stop`], it cannot be refused.
///
/// Distinct from [`stop`]: the stream keeps running (dropping an RTMP session
/// mid-incident is often worse), but what goes out is the privacy scene. The
/// human decides afterwards whether to stop entirely.
pub fn panic_cut(caller: Caller) -> LiveStatus {
    {
        let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
        st.privacy_cut = true;
        // A panic also revokes agent scene control — whatever went wrong, the
        // agent does not get to keep changing what is on air.
        st.agent_scene_control = false;
    }
    audit(caller, "panic_cut", json!({}));
    status()
}

/// Leave the privacy scene. Human-only: an agent must not be able to un-cut a
/// panic and put the room back on air.
pub fn clear_privacy_cut(caller: Caller) -> Result<LiveStatus, String> {
    if caller.is_agent() {
        return Err(format!(
            "{} may not clear the privacy cut; only a human can.",
            caller.label()
        ));
    }
    {
        let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
        st.privacy_cut = false;
    }
    audit(caller, "privacy_cut_cleared", json!({}));
    Ok(status())
}

// ------------------------------------------------- agent control while live

/// Rule 2/3: opt in to agent-driven scene / persona changes and preview→program
/// promotion **for this stream only**. Human-only, and [`stop`] clears it.
pub fn set_agent_scene_control(caller: Caller, allowed: bool) -> Result<LiveStatus, String> {
    if allowed && caller.is_agent() {
        return Err(format!(
            "{} may not grant itself scene control; only a human can, per stream.",
            caller.label()
        ));
    }
    {
        let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
        st.agent_scene_control = allowed;
    }
    audit(caller, "agent_scene_control", json!({ "allowed": allowed }));
    Ok(status())
}

/// Gate every agent-driven change to what is on air. Off-air, agents may mutate
/// a preview scene freely (shadow-mode DNA); on-air, they need the per-stream
/// opt-in. Human changes are always allowed. Every accepted change is audited.
pub fn authorize_scene_change(caller: Caller, what: &str) -> Result<(), String> {
    let (live, allowed, cut) = {
        let mut st = STATE.lock().unwrap_or_else(|e| e.into_inner());
        st.expire();
        (
            st.phase == Phase::Live,
            st.agent_scene_control,
            st.privacy_cut,
        )
    };
    if caller.is_agent() && live {
        if cut {
            audit(
                caller,
                "scene_change_denied",
                json!({ "what": what, "reason": "privacy cut engaged" }),
            );
            return Err("privacy cut is engaged; agents may not change what is on air.".into());
        }
        if !allowed {
            audit(
                caller,
                "scene_change_denied",
                json!({ "what": what, "reason": "no per-stream opt-in" }),
            );
            return Err(
                "agent scene control is off for this stream. A human must opt in per stream."
                    .into(),
            );
        }
    }
    audit(
        caller,
        "scene_change",
        json!({ "what": what, "live": live }),
    );
    Ok(())
}

// ----------------------------------------------------------------- audit log

/// Append one line to the rolling live-event log. Mirrors `terminal.rs::log_run`
/// (bounded, atomic) — this is the "visibly logged as they happen" half of
/// rule 2, and the record of who tried to start a stream.
fn audit(caller: Caller, event: &str, extra: Value) {
    let path = crate::paths::data_dir().join(LOG_FILE);
    let entry = json!({
        "at": crate::store::Store::now(),
        "caller": caller.label(),
        "event": event,
        "detail": extra,
    });
    let mut lines: Vec<String> = std::fs::read_to_string(&path)
        .map(|raw| raw.lines().map(String::from).collect())
        .unwrap_or_default();
    lines.push(entry.to_string());
    let keep = lines.len().saturating_sub(LOG_KEEP);
    let body = lines[keep..].join("\n");
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = crate::paths::write_atomic(&path, format!("{body}\n").as_bytes());
}

/// Recent live events, newest last. For the UI's in-session activity list.
pub fn recent_events(limit: usize) -> Vec<Value> {
    let path = crate::paths::data_dir().join(LOG_FILE);
    let raw = std::fs::read_to_string(&path).unwrap_or_default();
    let lines: Vec<&str> = raw.lines().filter(|l| !l.trim().is_empty()).collect();
    let start = lines.len().saturating_sub(limit.min(LOG_KEEP));
    lines[start..]
        .iter()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `STATE` is process-global, so tests that touch it must not interleave.
    /// Same pattern as `app_ui.rs`. `into_inner` on poison: one failing test
    /// should report its own assertion, not poison every later test.
    static LOCK: Mutex<()> = Mutex::new(());

    fn lock() -> std::sync::MutexGuard<'static, ()> {
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Presence that passes everything, so each test can spoil exactly one bit.
    fn present() -> Presence {
        Presence {
            cdp_enabled: false,
            physical_input_fresh: true,
            confirm_keys_down: true,
        }
    }

    const AGENT_CALLERS: [Caller; 4] = [
        Caller::Agent,
        Caller::Mcp,
        Caller::Automation,
        Caller::Workflow,
    ];

    #[test]
    fn no_agent_caller_can_arm_even_with_perfect_presence() {
        // The headline guarantee: grant on, human physically present, chord
        // held — and every agent surface is still refused.
        for caller in AGENT_CALLERS {
            let err = arm_gate(caller, true, &present())
                .expect_err("agent caller must never arm the go-live gate");
            assert!(
                err.contains("human-only"),
                "{caller:?} refused for the wrong reason: {err}"
            );
        }
        assert!(arm_gate(Caller::Human, true, &present()).is_ok());
    }

    #[test]
    fn provenance_is_checked_before_anything_else() {
        // An agent must be refused for *being an agent*, not because a grant
        // happened to be off — otherwise enabling the grant would open the path.
        for caller in AGENT_CALLERS {
            let err = arm_gate(caller, false, &present()).unwrap_err();
            assert!(err.contains("human-only"), "{caller:?}: {err}");
        }
    }

    #[test]
    fn grant_defaults_to_denied() {
        assert!(!LiveGrant::default().granted);
        let err = arm_gate(Caller::Human, false, &present()).unwrap_err();
        assert!(err.contains("not enabled"), "{err}");
    }

    #[test]
    fn cdp_enabled_blocks_going_live() {
        // With remote debugging open, any same-user process can synthesize the
        // clicks — so presence proves nothing and the gate must close.
        let p = Presence {
            cdp_enabled: true,
            ..present()
        };
        let err = arm_gate(Caller::Human, true, &p).unwrap_err();
        assert!(err.contains("remote debugging"), "{err}");
    }

    #[test]
    fn stale_machine_cannot_be_driven_live() {
        let p = Presence {
            physical_input_fresh: false,
            ..present()
        };
        let err = arm_gate(Caller::Human, true, &p).unwrap_err();
        assert!(err.contains("physical input"), "{err}");
    }

    #[test]
    fn confirm_chord_must_be_held() {
        let p = Presence {
            confirm_keys_down: false,
            ..present()
        };
        let err = arm_gate(Caller::Human, true, &p).unwrap_err();
        assert!(err.contains(CONFIRM_CHORD), "{err}");
    }

    #[test]
    fn agents_may_not_enable_the_grant() {
        for caller in AGENT_CALLERS {
            assert!(
                set_granted(caller, true).is_err(),
                "{caller:?} must not enable broadcasting"
            );
        }
    }

    #[test]
    fn stop_is_allowed_from_every_caller_and_cannot_fail() {
        let _g = lock();
        // Fail-safe direction: the type has no error to return.
        for caller in AGENT_CALLERS {
            let s: LiveStatus = stop(caller, "test");
            assert!(!s.live);
            assert_eq!(s.phase, Phase::Off);
        }
        let s = stop(Caller::Human, "test");
        assert!(!s.live);
    }

    #[test]
    fn panic_cut_is_allowed_from_every_caller() {
        let _g = lock();
        for caller in AGENT_CALLERS {
            let s = panic_cut(caller);
            assert!(s.privacy_cut, "{caller:?} must be able to cut to privacy");
        }
        // ...but only a human may put the room back on air.
        for caller in AGENT_CALLERS {
            assert!(clear_privacy_cut(caller).is_err());
        }
        assert!(clear_privacy_cut(Caller::Human).is_ok());
        stop(Caller::Human, "test cleanup");
    }

    #[test]
    fn agents_cannot_grant_themselves_scene_control() {
        let _g = lock();
        for caller in AGENT_CALLERS {
            assert!(set_agent_scene_control(caller, true).is_err());
        }
        // Turning it *off* is fine from anywhere — fail-safe direction.
        assert!(set_agent_scene_control(Caller::Agent, false).is_ok());
        stop(Caller::Human, "test cleanup");
    }

    #[test]
    fn scene_changes_off_air_are_free_but_on_air_need_opt_in() {
        let _g = lock();
        stop(Caller::Human, "reset");
        // Off air: shadow-mode DNA, agents mutate preview freely.
        assert!(authorize_scene_change(Caller::Agent, "preview scene").is_ok());

        // Force the live phase directly: this test is about the scene rule, and
        // go_live() needs a real keyboard.
        {
            let mut st = STATE.lock().unwrap();
            st.phase = Phase::Live;
            st.live_since = Some(Instant::now());
            st.agent_scene_control = false;
            st.privacy_cut = false;
        }
        let err = authorize_scene_change(Caller::Agent, "cut to camera 2").unwrap_err();
        assert!(err.contains("opt in"), "{err}");
        // A human at the desk is never blocked by the opt-in.
        assert!(authorize_scene_change(Caller::Human, "cut to camera 2").is_ok());

        set_agent_scene_control(Caller::Human, true).unwrap();
        assert!(authorize_scene_change(Caller::Agent, "cut to camera 2").is_ok());

        // Panic revokes it mid-stream.
        panic_cut(Caller::Human);
        let err = authorize_scene_change(Caller::Agent, "cut back").unwrap_err();
        assert!(err.contains("privacy cut"), "{err}");

        stop(Caller::Human, "test cleanup");
    }

    #[test]
    fn stop_clears_the_per_stream_opt_in() {
        let _g = lock();
        set_agent_scene_control(Caller::Human, true).unwrap();
        assert!(status().agent_scene_control);
        stop(Caller::Human, "end of stream");
        assert!(
            !status().agent_scene_control,
            "scene control must not survive into the next stream"
        );
    }

    #[test]
    fn go_live_requires_arming_first() {
        let _g = lock();
        stop(Caller::Human, "reset");
        let err = go_live(Caller::Human).unwrap_err();
        assert!(err.contains("not armed"), "{err}");
    }

    #[test]
    fn agent_go_live_is_refused_even_when_armed() {
        let _g = lock();
        stop(Caller::Human, "reset");
        {
            let mut st = STATE.lock().unwrap();
            st.phase = Phase::Armed;
            st.armed_at = Some(Instant::now());
        }
        for caller in AGENT_CALLERS {
            let err = go_live(caller).unwrap_err();
            assert!(
                err.contains("human-only"),
                "{caller:?} must not ride a human's arm: {err}"
            );
        }
        stop(Caller::Human, "test cleanup");
    }

    #[test]
    fn a_stale_arm_expires_instead_of_lingering() {
        let _g = lock();
        stop(Caller::Human, "reset");
        {
            let mut st = STATE.lock().unwrap();
            st.phase = Phase::Armed;
            // Older than ARMED_TTL.
            st.armed_at = Instant::now().checked_sub(ARMED_TTL + Duration::from_secs(1));
        }
        let err = go_live(Caller::Human).unwrap_err();
        assert!(err.contains("not armed"), "expired arm must not work: {err}");
    }

    #[test]
    fn confirm_without_a_request_is_refused() {
        let _g = lock();
        stop(Caller::Human, "reset");
        let err = confirm_arm(Caller::Human, "made-up-nonce").unwrap_err();
        assert!(err.contains("no active go-live request"), "{err}");
    }

    #[test]
    fn a_nonce_is_single_use() {
        let _g = lock();
        stop(Caller::Human, "reset");
        {
            let mut st = STATE.lock().unwrap();
            st.challenge = Some(("nonce-a".into(), Instant::now()));
        }
        // Wrong value consumes the challenge...
        assert!(confirm_arm(Caller::Human, "nonce-b").is_err());
        // ...so even the right value cannot be replayed afterwards.
        let err = confirm_arm(Caller::Human, "nonce-a").unwrap_err();
        assert!(err.contains("no active go-live request"), "{err}");
    }

    #[test]
    fn agent_callers_cannot_request_a_challenge() {
        for caller in AGENT_CALLERS {
            let err = request_arm(caller).unwrap_err();
            assert!(err.contains("human-only"), "{caller:?}: {err}");
        }
    }

    #[test]
    fn status_is_safe_to_show_anyone() {
        // Agents *should* see that a stream is running — that is what lets them
        // stop one. What they must never see is a credential or a destination.
        // Assert the exact field set so adding a stream key or ingest URL here
        // fails loudly rather than quietly widening what a webview can read.
        let _g = lock();
        let raw = serde_json::to_value(status()).unwrap();
        let mut fields: Vec<&str> = raw.as_object().unwrap().keys().map(|k| k.as_str()).collect();
        fields.sort_unstable();
        assert_eq!(
            fields,
            [
                "agentSceneControl",
                "blockedReason",
                "confirmChord",
                "granted",
                "live",
                "liveSeconds",
                "phase",
                "privacyCut",
            ],
            "live status shape changed — check nothing sensitive was added"
        );
    }
}

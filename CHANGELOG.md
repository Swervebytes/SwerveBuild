# Changelog

All notable changes to Swerve Build. The format follows
[Keep a Changelog](https://keepachangelog.com/); versions are `MAJOR.MINOR.PATCH`.
History before the first public release is summarized coarsely — the granular
record lives in the git log.

## [Unreleased]

### Added
- **Provider sign-in from Settings** — Claude Code (`claude /login` in a
  terminal) and Gemini (Google OAuth) sign in through each agent's own flow;
  chats that hit a signed-out agent show a banner pointing at the fix.
- **Automations honesty** — the page states that automations run on Grok when
  another provider is active, and that they can't run when Grok isn't installed.
- Long chats render the newest 150 messages with a "Show earlier" control
  instead of every bubble ever.

### Changed
- **All secrets now live in Windows Credential Manager** — the custom-endpoint
  API key and local llama-server token migrate out of `providers.json`
  automatically on first launch.
- A real Content-Security-Policy ships (was previously unset).
- Daily/weekly automation schedules follow the OS clock across DST transitions
  (previously drifted an hour until re-saved).

### Fixed
- ACP peers that use string JSON-RPC ids no longer have permission requests
  silently dropped.
- Live output produced in the first instant of a "Run now" is no longer lost.
- Agent probes no longer leak node processes on Windows (tree-kill of npm
  `.cmd` shims).

## [0.3.x] — 2026-07 → 2026-08 (pre-public development line)

One session at a time, verify-then-merge. Highlights:

- **Screen recorder** (0.3.19–0.3.20): real display capture → H.264 MP4 via
  NVENC with software fallback, desktop audio via WASAPI loopback.
- **Context bar** (0.3.21–0.3.23): honest per-chat token usage — real counts
  from the agent wire, `—` when the agent reports nothing.
- **OS keystore seam** (0.3.24): Windows Credential Manager for stream keys;
  no command can read a secret back.
- **Providers managed in-app** (0.3.25): install/uninstall Claude Code and
  Gemini at pinned versions with confirm-before-remove.
- **Go-live approval tier** (0.3.26–0.3.27): a broadcast permission no agent
  path can satisfy (streaming itself lands in a future release).
- Workflow engine (Rust + QuickJS node graphs), browser debug pane, terminal
  tools, app-UI drive — each behind an explicit off-by-default grant.

## [0.2.x] — 2026-07

- Triggered automations (schedule / git / file / manual) with shadow-mode
  read-only safety, run transcripts, and chaining.
- Multi-provider ACP support, custom Grok endpoint (BYOK / local inference),
  local GGUF models via managed llama-server.

## [0.1.x] — 2026-06

- Initial shell: project chats over ACP with streaming, tool approval,
  memories, skills, MCP sidecar.

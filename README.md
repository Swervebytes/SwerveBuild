# Swerve Build

A clean, minimal Windows desktop app for [Grok Build CLI](https://github.com/xai-org/grok-cli) and other ACP agents. Project-scoped chats, tool approval, memories, skills, and multi-provider support — in one native shell.

**License:** MIT

## Install (users)

1. Download the latest **Windows installer** from [Releases](https://github.com/Swervebytes/SwerveBuild/releases) (`Swerve Build_*_x64-setup.exe`).
2. Run the installer and open **Swerve Build**.
3. On the home screen, click **Install Grok Build** (installs the official Grok CLI to `~/.grok/`).
4. Click **Sign In** and complete OAuth in your browser.
5. Open **Projects**, add a folder, and start a chat.

No Node.js or Rust required for end users.

### Using other agents

Open **Settings → Providers** and pick an available agent:

| Provider | How to enable |
|----------|----------------|
| **Grok** (default) | Install + sign in on the home screen |
| **Claude Code** | Install [claude-code-acp](https://www.npmjs.com/package/claude-code-acp) and ensure it is on your PATH |
| **Gemini** | Install the Gemini CLI and ensure `gemini` is on your PATH |
| **Ollama / OpenAI / Anthropic** | Listed as designed — HTTP chat is not spawnable yet |

The home screen shows your **active provider** and whether it is ready.

## Features

- **Home** — Grok install, OAuth sign-in, update check, active provider status
- **Projects** — folder-scoped repos and chats (persisted in `~/.swervebuild/`)
- **Chat** — ACP streaming, image paste, tool approval UI, up to 3 concurrent sessions
- **Memories** — edit Grok's global `MEMORY.md`
- **Skills** — browse installed Grok skills
- **Settings** — theme and provider picker
- **MCP sidecar** — `swervebuild-mcp` exposes app state to agents in chat

## Develop

### Prerequisites

1. [Node.js](https://nodejs.org/) 18+
2. [Rust](https://www.rust-lang.org/tools/install) (`rustup`)
3. [Tauri prerequisites for Windows](https://tauri.app/start/prerequisites/) — WebView2 and MSVC build tools

### Quick start

```powershell
git clone https://github.com/Swervebytes/SwerveBuild.git
cd SwerveBuild
npm install
npm run tauri dev
```

> Do **not** double-click `target/debug/*.exe` directly — dev mode needs the Vite server.

### Scripts

```powershell
npm run dev          # Frontend only (browser)
npm run check        # Type-check Svelte
npm run test:e2e     # CLI tests (MCP always; ACP if grok + SWERVE_E2E_CWD set)
npm run release      # Production installer (Windows)
npm run tauri dev    # Full desktop app with hot reload
```

### Build installer (maintainers)

```powershell
npm run release
```

Output: `src-tauri/target/release/bundle/nsis/Swerve Build_0.1.0_x64-setup.exe`

Upload the `*-setup.exe` to GitHub Releases. The script builds the frontend and Rust binaries, stages the MCP sidecar, bundles with Tauri, then restores dev-friendly config.

## Data locations

| Path | Purpose |
|------|---------|
| `~/.swervebuild/data.json` | Projects, chats, messages |
| `~/.swervebuild/providers.json` | Active provider preference |
| `~/.swervebuild/attachments/` | Pasted chat images |
| `~/.grok/` | Grok CLI (auth, skills, memory, sessions) |

On first launch, existing `~/.swervegrok/` data is migrated automatically to `~/.swervebuild/`.

## Stack

- [Tauri 2](https://tauri.app/) — native binary
- [SvelteKit](https://svelte.dev/) + Svelte 5 — SPA frontend
- Rust — ACP session pool, persistence, Grok CLI integration

## Project structure

```
src/                  SvelteKit UI
src-tauri/            Rust backend (Tauri + ACP + MCP)
  src/acp.rs          Multi-chat session pool
  src/paths.rs        App data dir + legacy migration
  src/providers.rs    Multi-agent provider registry
  src/bin/            swervebuild-mcp stdio server
scripts/              Release build helper
```

## Contributing

Contributions welcome. Keep the UI clean and dependencies minimal. Open an issue before large architectural changes.

Maintainers: read [`.github/REPOSITORY_POLICY.md`](.github/REPOSITORY_POLICY.md) before pushing or cutting releases. Dependabot handles npm and Cargo updates weekly.
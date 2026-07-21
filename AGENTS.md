# Agent instructions (Swerve Build)

Read `.github/REPOSITORY_POLICY.md` before any git push or release.

## Non-negotiable rules

1. **Never add Grok/xAI as a GitHub collaborator** on this repo.
2. **Never commit** local paths, user data, secrets, build output, or Grok session files.
3. **Never** mark the project as “built by Grok” — it is built **for** Grok Build CLI integration.
4. Update repo URLs to `https://github.com/Swervebytes/SwerveBuild` when changing package metadata.

## Before pushing

- Grep for `E:\`, `C:\Users`, and personal emails in tracked files.
- Confirm `.gitignore` excludes `target/`, `binaries/`, `build/`, `.env*`.
- Run `npm run check` and `npm run test:e2e` (set `SWERVE_E2E_CWD` for full ACP tests).

## Stack

- Tauri 2 + SvelteKit 2 + Svelte 5 + Rust
- Dev: `npm run tauri dev` (not raw `target/debug/*.exe`)
- Release: `npm run release` → NSIS installer under `src-tauri/target/release/bundle/nsis/`
- **Local ship (session-close):** if the desktop app changed, run `npm run install:local` so the installed app matches this session. Optional: `npm run install:local -- -Bump patch` (or `minor` / `major`) first. Versions must stay in sync across `package.json`, `src-tauri/Cargo.toml`, and `src-tauri/tauri.conf.json` (`npm run version:check`).
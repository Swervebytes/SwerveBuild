# Swerve Build — Repository Policy

Hard rules for anyone publishing or maintaining this public repository.

## Access & collaborators

- **Never add Grok, xAI, or Grok Build as a GitHub collaborator, org member, or bot with write access** on this repository.
- Swerve Build is an independent open-source project **for** the Grok Build ecosystem, not **by** or **owned by** Grok/xAI.

## What must not be committed

- Local drive paths (`E:\`, `C:\Users\...`, machine-specific folders)
- User data (`~/.swervebuild/`, `~/.swervegrok/`, `~/.grok/`)
- Build artifacts (`src-tauri/target/`, `src-tauri/binaries/`, `build/`, `node_modules/`)
- API keys, tokens, `auth.json`, or session files
- IDE-only configs (`.vscode/`, `.claude/`, `.idea/`)
- Grok session exports, chat logs, or agent transcripts from local development

## Branding & metadata

- Describe the project as a **desktop shell for Grok Build CLI** (and other ACP agents).
- Do **not** label releases, README, or package metadata as “built by Grok” or imply xAI authorship.
- Keep `LICENSE` copyright as **Swerve Build Contributors** (or Swervebytes).

## Dependencies & hygiene

- Dependabot is enabled for **npm** and **Cargo** — review and merge security updates promptly.
- Run `npm run check` and `npm run test:e2e` before tagging releases.
- Upload installers to GitHub Releases only; never commit `.exe` / `.msi` bundles to git.

## Releases

- Windows installer is built with `npm run release` on a maintainer machine or CI.
- macOS/Linux ports are out of scope for v0.1.0; track separately when ready.
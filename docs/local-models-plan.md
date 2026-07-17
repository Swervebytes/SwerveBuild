# Local models plan — first-class model selection, no Ollama required

**Vision (from Swerve):** Swerve Build is a standalone, one-click app. Projects work no
matter the model chosen. It's easy to change model in a chat or when setting a trigger.
Adding a local model shows what's available to download, asks for a folder, sets
everything up, and the model then appears as a choice in chats and triggers.

**Prime directive: improve what we have, don't break it.** Every phase ships
independently, defaults to today's behavior when nothing is selected, and never removes
an existing surface.

---

## Architecture (checked and corrected)

**Grok Build stays the one agent harness.** A "model" is just a `[model.<name>]` entry
in `~/.grok/config.toml` — hosted (grok-4, grok-code-fast…) or local (a GGUF served by
the app's own inference server). Projects/tools/approvals/memories behave identically
for every model because the harness never changes. Agents (Claude Code, Gemini) remain
a separate *provider* concept — the model picker lives inside the Grok provider.

**Verified 2026-07-16 against the installed CLI (`grok agent --help`, v0.2.93):**
`grok agent` accepts `-m, --model <MODEL>`. Per-chat models = append `["-m", id]` to
the ACP spawn args (`AcpLaunch.args`). Headless (`grok` root) also takes `-m` →
per-trigger models use the same mechanism in the automation runner. This removes the
plan's biggest unknown — no dependency on ACP `session/set_model` support.

**Local inference engine: llama.cpp `llama-server`** (MIT, official Windows prebuilt
with Vulkan — works on any GPU vendor, no compilation). Fetched at runtime like the
Grok CLI is (NOT bundled as a Tauri sidecar — sidecars are for build-time binaries;
runtime fetch keeps the installer lean and matches the existing "Install Grok Build"
pattern). Spawned with the same `hidden_command` machinery as grok itself.

**The shipped custom-endpoint feature evolves into this** — it becomes "one entry in
the model registry" while its existing Settings card keeps working as the advanced
global override (useful for air-gapped users who also want the standalone `grok` CLI
routed). `grok_config.rs` generalizes from managing one block to managing N
`swerve-*`-prefixed blocks; its transform/backup/restore machinery and tests carry
over directly.

---

## Non-regression contract

These must remain true after every phase:

1. **No selection = today's behavior.** A chat/trigger with no model chosen spawns
   grok exactly as now (no `-m` appended). Fresh installs behave identically.
2. **Old data loads.** New fields (`Chat.model_id`, `Automation.model`) are
   `Option` + serde defaults; existing `data.json` / `automations.json` /
   `providers.json` parse unchanged (quarantine-on-corrupt already exists).
3. **We only touch our namespace** in `~/.grok/config.toml`: blocks named
   `swerve-*`. Never modify/delete a user's own `[model.X]`. (Extend the existing
   test suite to prove it.)
4. **Custom-endpoint card keeps working** — no removal, no behavior change to the
   global toggle.
5. **Claude Code / Gemini providers unaffected** — picker is scoped to Grok.
6. **Automations safety unchanged** — model choice is orthogonal to shadow-mode /
   read-only tool gating.
7. **Clean exit** — llama-server dies on app exit (extend the existing
   `RunEvent::Exit` cleanup alongside acp/jobs).
8. **Graceful failure** — engine missing/crashed → clear error on that chat only;
   hosted chats never affected.
9. **Existing checks pass** per phase: `npm run check`, `cargo test`,
   `npm run test:e2e` (with `SWERVE_E2E_CWD`), plus a live-app drive of the touched
   surface (GROK_HOME-isolated where config is involved).

---

## Phases

### Phase 0 — Stabilize the base (small, do first)
- Run `test:e2e` on `feat/custom-grok-endpoint` (the one unchecked box from that
  session); fix anything it surfaces; merge to `main`.
- Commit this plan doc.
- Leave the unrelated WIP (`acp.rs` swervebytes-mcp, `package-lock.json`, `scripts/`)
  untouched, as before.
- ~~Spike: confirm `-m` on `grok agent`~~ ✅ done (see above). Remaining micro-spikes,
  folded into later phases: (a) Phase 1 — observe whether hosted model IDs work via
  `-m` without config entries; (b) Phase 2 — hard-verify a chat works with an
  unauthenticated GROK_HOME against a local server (the "no sign-in" claim).

### Phase 1 — Model registry + per-chat/per-trigger selection (no engine yet)
The UX skeleton, shippable on its own (lets users pick hosted models per chat today):
- `grok_config.rs`: manage N `[model.swerve-*]` blocks (registry-driven); keep the
  single-endpoint API as a thin wrapper.
- Registry in `providers.json`: hosted model list (curated IDs + user-addable) and
  custom entries; `Chat.model_id`, `Automation.model`.
- `acp.rs` spawn + `jobs.rs` `build_grok_args`: append `-m <id>` when set.
- Shared `ModelPicker.svelte` in `ChatHeader` and the automation editor.
- **Change-model-in-chat included here** (it's core to the vision): on change,
  close the child, respawn with the new `-m`, and `session/load` the same session ID —
  the replay/suppress machinery for this already exists and works. Conversation
  continuity preserved; feels like an in-place switch.
- Acceptance: two chats on different hosted models concurrently; a trigger pinned to
  a model; unset = unchanged behavior; config file diff shows only `swerve-*` blocks.

### Phase 2 — Managed local engine (file-picker first)
Prove local inference end-to-end before any download UX:
- Engine fetch on first use: **pinned** llama.cpp release tag + SHA-256 recorded at
  pin time; Vulkan x64 asset; unzip to `~/.swervebuild/engine/<ver>/` (new `zip` +
  download deps).
- `LlamaServerManager` (new `local_llm.rs`): start/stop/health-poll (model load can
  take 30–120 s — status events to UI), crash detection, kill on exit.
  Flags that matter for *agentic* use: `--host 127.0.0.1` (never 0.0.0.0),
  `--jinja` (required for OpenAI-style tool calling), generous `-c` split across
  `--parallel 2` slots, `-ngl 999` with CPU fallback, `--api-key <generated token>`
  (injected to grok via the existing env_key mechanism — machinery already shipped),
  `--model-alias <slug>`, `--no-webui`.
- Port: pick a free port once, persist it; config entries stay stable (rewrite only
  if the port ever changes).
- Policy v1: **one local model loaded at a time**; selecting another swaps the
  server; if a local-model chat/run is mid-generation, block the swap with a clear
  message.
- "Add local model" via **file picker** for an existing GGUF → registers
  `[model.swerve-local-<slug>]` → appears in every picker.
- Automations: runner ensures the server is up (with timeout → `launchfailed` and a
  clear message) before a local-model run.
- Acceptance: full chat + a shadow automation on a local GGUF, **signed-out**
  (throwaway GROK_HOME) — proving the standalone/no-xAI path.

### Phase 3 — Catalog + downloads ("here's what's available")
The one-click flow from the vision:
- Curated in-app catalog (3–5 entries, refreshed at build time): agentic-capable
  coder models with permissive licenses (Qwen-coder class, DeepSeek, Devstral —
  finalize current best when this phase starts), each with size, VRAM needs, quant,
  context, and an honest "what it's good at" note.
- Hardware match: detect VRAM (`nvidia-smi` when present; conservative tiers
  otherwise) and mark each entry *Fits / Tight / Too big*, with CPU-only warnings.
- Folder prompt (default `~/.swervebuild/models/`, persisted), free-space check,
  direct HuggingFace download (no gated models in v1) with progress/speed/ETA and
  **resume** via Range requests (files are 8–20 GB; non-resumable is not acceptable).
- On completion: auto-register → model appears in pickers. Delete/manage UI.
- Acceptance: fresh machine → download → selected in a chat and a trigger with no
  manual steps beyond choosing the folder.

### Phase 4 — Polish
- `--reasoning-effort` in the picker for hosted reasoning models (flag exists).
- Optional CUDA engine variant for NVIDIA speed; engine update flow.
- ACP `session/set_model` (instant switch, no respawn) if/when Grok advertises it.
- README + screenshots: "bring your own model — no Ollama, no sign-in required."
- Optional: surface the local server as an endpoint other tools can use (we become
  the local inference provider on the machine).

---

## Risk table

| Risk | Mitigation |
|---|---|
| Small models flail in an agent harness | Curated agentic-capable catalog, hardware-matched sizes, honest capability notes; hosted models always one click away |
| Tool-calling breaks on local models | `--jinja` + template-aware catalog picks; Phase 2 acceptance includes a real tool-using chat |
| Engine download breaks (AV, network) | Official GitHub release, pinned tag + checksum, clear errors, retry; runtime-fetch keeps installer clean |
| VRAM contention / model swap races | One-local-model policy + busy-block with message |
| Clobbering user's grok config | `swerve-*` namespace rule + tests; backup-once already shipped |
| Port collisions | Persisted free-port selection, re-scan on failure |
| grok requires sign-in even for local | Auth hierarchy says env_key suffices; hard-verified in Phase 2 acceptance; fallback = document sign-in as one-time setup |
| Big context OOMs GPU | Catalog ctx recommendations per tier; `-c` conservative defaults, user-tunable later |

## Open items (non-blocking, decide at the phase)
- Phase 1: initial hosted-model ID list (curate from grok's own docs/TUI at build time).
- Phase 3: catalog contents + default models folder location.
- Naming: "Local models" vs "My models" in the UI.

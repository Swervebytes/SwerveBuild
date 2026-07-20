# Swerve Workflows — engine design

**Status:** IMPLEMENTED 2026-07-20 (engine + app integration + canvas editor, branch `feat/workflows`). §14 records where the build deviates from this spec — otherwise the doc matches the code.
**Decisions locked (prior session):** Rust engine + embedded QuickJS for expressions and the Code node · n8n-style items data model · engine-first, headless — the executor runs workflow JSON files with zero UI before any canvas work starts · editor (later) = Svelte Flow (`@xyflow/svelte`) · shipped Automations stay untouched.

This document specifies the three things that must be right before code: the **workflow JSON schema**, the **node interface**, and the **execution model**. Everything else (canvas, palette UX, automation convergence) builds on these and gets its own doc later.

---

## 1. Goals and non-goals

**Goals**

- A general-purpose node-graph workflow engine inside SwerveBuild: triggers, HTTP, transforms, conditionals, code, and agent runs as *one node type among many*.
- Deterministic, inspectable execution: same workflow + same inputs → same node order, with a durable per-run record you can replay in the UI.
- Structural safety in Rust, continuing the Automations posture: capability comes from permission-checked services, never from node code discipline, and hard caps cannot be lifted by editing JSON on disk.
- Engine testable without Tauri: a pure Rust crate, driven by integration tests and a headless runner binary.

**Non-goals for v1** (explicitly out; revisit after the engine proves out)

- Webhook/HTTP-server triggers (the app is a desktop shell; no listening sockets yet).
- Loops / cycles in the graph (DAG only; a dedicated Loop node can come later).
- Binary data items (images, files-as-items). The schema reserves room; v1 items are JSON only.
- Credentials in the OS keyring (v1 has a simpler secrets file; see §8.5).
- Parallel execution of branches inside one run (runs are single-threaded; concurrency is across runs).
- Sub-workflows, shell node, editor canvas (M3), automation migration (M4).

---

## 2. Architecture overview

### 2.1 Crate layout — Tauri-independence enforced by the compiler

The engine is a workspace member crate with **zero Tauri dependencies**. The app crate and the headless runner both consume it. This makes "engine-first headless" structural instead of aspirational, the same way shadow-mode tool intersection is structural in `jobs.rs`.

```
src-tauri/
  Cargo.toml                      workspace root; app crate + members
  crates/swerve-workflows/        the engine crate (no tauri, no app types)
    src/
      lib.rs
      model.rs                    Workflow / Node / Connection / Permissions (serde)
      items.rs                    Item, port collections
      validate.rs                 load-time + pre-run validation
      engine.rs                   the executor (ready queue, delivery, supervisor)
      expr.rs                     QuickJS expression + template resolution
      nodes/                      one file per built-in node
      permissions.rs              network / fs / code / agent gates
      runs.rs                     RunRecord, event log writer, pruning
      error.rs
    tests/                        integration tests + example workflow JSONs
  src/workflows_tauri.rs          Tauri adapter: commands, event bridge, scheduler wiring
  src/bin/swervebuild_workflow.rs headless CLI runner (mirrors the existing MCP bin pattern)
```

Dependencies added: `rquickjs` (bundled QuickJS) and `ureq` (rustls). Both are deliberate lean picks; §11 covers the MSVC build risk and the fallback seam.

### 2.2 What the engine borrows from `jobs.rs` (proven patterns, reused not rewritten)

| Pattern | Source | Reuse |
|---|---|---|
| Versioned store, `migrate()` hook, `#[serde(flatten)] extra` forward-compat | `AutomationStore` | identical pattern on the workflow file |
| Atomic write + corrupt-file quarantine | `paths.rs` | identical |
| Trigger semantics: schedule math, git head polling, file snapshot + settle tick | scheduler in `jobs.rs` | logic ported into trigger nodes; bookkeeping moves to workflow `state` |
| Run records: `<run>.json` meta + `<run>.jsonl` append-only log, prune keep-N | run-record I/O | identical shape, new directory |
| Process discipline: hidden command, stdin null, tree-kill, cooperative cancel flag, bounded channel collection | `spawn_run` | Agent node and run supervisor |
| Admission control: pause, overlap skip/replace, capacity queue, wake channel | `JobManager` | `WorkflowManager`, same design |
| Structural safety: enforcement in Rust at a choke point, never in stored JSON | `build_grok_args` | `permissions.rs` services (§8) |

The Automations scheduler thread is **not** modified. Workflows get their own scheduler thread with the same 30-second tick + wake-channel design. Zero risk to the shipped feature; unification is an M4 decision.

### 2.3 Injected services — the engine's only doors to the outside

The engine crate defines traits; the app injects implementations. Tests inject fakes.

```rust
pub struct EngineServices {
    /// Runs a headless agent turn. The app injects the grok implementation
    /// (same invocation discipline as jobs.rs). None = agent node unavailable.
    pub agent: Option<Arc<dyn AgentRunner>>,
    /// Named secret lookup for {{ $secret("name") }}.
    pub secrets: Arc<dyn SecretStore>,
    /// Run/node lifecycle events. The app forwards these as Tauri emits;
    /// tests collect them; the CLI prints them.
    pub events: Arc<dyn RunEvents>,
}

pub trait AgentRunner: Send + Sync {
    fn run(&self, req: AgentRequest, cancel: &AtomicBool) -> Result<AgentResult, String>;
}

pub trait SecretStore: Send + Sync {
    fn get(&self, name: &str) -> Option<String>;
}

pub trait RunEvents: Send + Sync {
    fn emit(&self, event: RunEvent);
}
```

HTTP and filesystem access are engine-owned (implemented inside `permissions.rs`-gated services), not injected — their safety checks are part of the engine's guarantees and must not vary by host.

---

## 3. Storage layout

```
~/.swervebuild/
  workflows/<workflow_id>.json          one file per workflow (id = "w-<uuid>")
  workflow-runs/<workflow_id>/
    <run_id>.json                       RunRecord (final + progressively updated)
    <run_id>.jsonl                      append-only event log
  secrets.json                          v1 secret store (§8.5)
```

One file per workflow (unlike the single `automations.json`) because workflows are documents: they're bigger, they're edited as a unit, the headless runner takes a file path, and a user can copy or version-control one workflow without touching the rest. Listing = directory scan; ids are validated with the existing `is_safe_id` discipline before any path join.

All writes go through `write_atomic`; unparseable files are quarantined, never overwritten.

---

## 4. Workflow JSON schema

### 4.1 Complete example

A real workflow — nightly repo digest: schedule trigger → shadow agent summarizes the day's commits → IF the agent didn't reply SILENT → write a report file.

```json
{
  "version": 1,
  "id": "w-8f14e45f-ceea-467a-9575-0123456789ab",
  "name": "Nightly repo digest",
  "enabled": true,
  "project_id": null,
  "settings": {
    "timeout_secs": 900,
    "overlap": "skip",
    "min_interval_secs": 0,
    "keep_runs": 50,
    "capture": "sample",
    "max_items_per_port": 10000
  },
  "permissions": {
    "network": { "enabled": false, "hosts": [], "private_ips": false },
    "code": false,
    "fs": { "read": [], "write": ["E:/SwerveGrokBuild/reports"] },
    "agent": true
  },
  "nodes": [
    {
      "id": "n1",
      "type": "trigger.schedule",
      "type_version": 1,
      "name": "Every night",
      "position": [80, 200],
      "disabled": false,
      "notes": "",
      "on_error": "stop",
      "retry": null,
      "params": {
        "every": "daily", "hour": 21, "minute": 0, "tz_offset_minutes": 300
      }
    },
    {
      "id": "n2",
      "type": "agent.run",
      "type_version": 1,
      "name": "Summarize commits",
      "position": [360, 200],
      "disabled": false,
      "notes": "Shadow mode; reads git log via read-safe tools.",
      "on_error": "stop",
      "retry": { "attempts": 1, "backoff_secs": [30] },
      "params": {
        "cwd": "E:/SwerveGrokBuild",
        "prompt": "Summarize the commits made in the last 24 hours in this repo. Reply with exactly SILENT if there are none.",
        "max_turns": 15,
        "timeout_secs": 600,
        "web_search": false,
        "json_schema": null,
        "model": null,
        "effort": null
      }
    },
    {
      "id": "n3",
      "type": "flow.if",
      "type_version": 1,
      "name": "Anything to report?",
      "position": [640, 200],
      "disabled": false,
      "notes": "",
      "on_error": "stop",
      "retry": null,
      "params": {
        "combine": "and",
        "conditions": [
          { "left": "{{ $json.text }}", "op": "ne", "right": "SILENT" }
        ]
      }
    },
    {
      "id": "n4",
      "type": "file.write",
      "type_version": 1,
      "name": "Write report",
      "position": [920, 120],
      "disabled": false,
      "notes": "",
      "on_error": "stop",
      "retry": null,
      "params": {
        "path": "E:/SwerveGrokBuild/reports/digest-{{ $run.started_at }}.md",
        "content": "{{ $json.text }}",
        "mode": "overwrite"
      }
    }
  ],
  "connections": [
    { "from": "n1", "out": "main", "to": "n2", "in": "main" },
    { "from": "n2", "out": "main", "to": "n3", "in": "main" },
    { "from": "n3", "out": "true", "to": "n4", "in": "main" }
  ],
  "state": {
    "last_fired_at": null,
    "last_run_id": null,
    "last_status": null,
    "trigger": {}
  },
  "created_at": "1784500000",
  "updated_at": "1784500000"
}
```

### 4.2 Top-level fields

| Field | Type | Notes |
|---|---|---|
| `version` | u32 | store schema version; `migrate()` hook branches on it, exactly like `AutomationStore` |
| `id` | string | `w-<uuid>`, generated on first save, `is_safe_id`-valid |
| `name` | string | display name |
| `enabled` | bool | disabled workflows never trigger (manual run still allowed, mirroring the Automations pause-vs-manual rule) |
| `project_id` | string? | optional association with an app project |
| `settings` | object | run behavior, §4.3 |
| `permissions` | object | capability grants, §8 |
| `nodes` | array | §4.4 |
| `connections` | array | §4.5 |
| `state` | object | runtime bookkeeping owned by the scheduler/engine: `last_fired_at`, `last_run_id`, `last_status`, and `trigger` — a map of trigger-node id → its persisted bookkeeping (`last_seen_commit`, file `snapshot`), replacing the fields that live inside `Trigger` variants in Automations |
| `created_at` / `updated_at` | string | epoch-secs strings, matching `Store::now()` |
| *(flattened)* `extra` | map | `#[serde(flatten)]` — unknown fields written by a newer app version round-trip losslessly |

### 4.3 `settings`

| Field | Default | Notes |
|---|---|---|
| `timeout_secs` | 600 | wall-clock cap for the whole run, enforced by the supervisor (§7.6) |
| `overlap` | `"skip"` | `skip` \| `replace`, same semantics as Automations |
| `min_interval_secs` | 0 | trigger rate limit (git/file triggers) |
| `keep_runs` | 50 | prune threshold, newest kept |
| `capture` | `"sample"` | run-data capture: `"sample"` (first 20 items per port, ≤256 KB per node) \| `"full"` \| `"none"` — §7.7 |
| `max_items_per_port` | 10000 | hard cap; a node emitting more fails with a clear error (memory guard) |

### 4.4 Node object

| Field | Type | Notes |
|---|---|---|
| `id` | string | unique within the workflow; stable across edits (connections reference it) |
| `type` | string | registry key, namespaced: `trigger.schedule`, `http.request`, `flow.if`, … |
| `type_version` | u32 | always 1 for now. Reserved for breaking param-shape changes; the registry resolves `(type, type_version)`. Additive param changes use serde defaults instead |
| `name` | string | user label, unique within the workflow (validated) — expressions reference nodes by name (§6.3) |
| `position` | [f64, f64] | canvas coordinates; engine ignores it |
| `disabled` | bool | pass-through: first input port forwarded verbatim to first output port, other outputs empty |
| `notes` | string | annotation shown on canvas |
| `on_error` | string | `"stop"` (default) \| `"skip"` \| `"branch"` — §7.5 |
| `retry` | object? | `{ attempts, backoff_secs }`, whole-node retry before `on_error` applies |
| `params` | object | node-specific configuration; string values may contain `{{ }}` templates (§6) |

### 4.5 Connection object

A flat edge list (deliberately not n8n's nested `connections[name][index][][]` maps — flat is easier to validate, diff, and edit by hand):

```json
{ "from": "n3", "out": "true", "to": "n4", "in": "main" }
```

Ports are **named strings**, not indexes. Conventions: the default port on both sides is `"main"`; `flow.if` outputs `"true"` / `"false"`; `flow.merge` inputs `"a"` / `"b"`; `on_error: "branch"` adds a virtual output `"error"` to any node. Multiple edges may leave one output (each target gets the full item set) and multiple edges may enter one input (deliveries concatenate, in deterministic edge order — the order they appear in `connections`).

### 4.6 Validation (at save and again before every run)

- Node ids unique; node names unique and non-empty; connection endpoints exist; port names exist on the referenced node's spec (`error` allowed only when `on_error: "branch"`).
- **The graph is a DAG** — cycles are a hard error in v1.
- At least one trigger node; trigger nodes have no input ports.
- Non-trigger nodes with no incoming connection are flagged (warning at save; skipped at run — unreachable).
- Params structurally checked against the node's params spec (unknown params warn, don't fail — forward compat).
- Every node's declared capability needs (§5.2) are covered by `permissions` — violations listed by node, run refused with a precise message ("HTTP Request 'Fetch board' needs network access to api.example.com").

---

## 5. Node interface

### 5.1 Data model — items

```rust
/// One unit of data flowing along an edge. v1 is JSON-only; a `binary` field
/// (attachment-store references) is the planned extension, which is why Item
/// is a struct and not a bare Value.
#[derive(Clone, Serialize, Deserialize)]
pub struct Item {
    pub json: serde_json::Value,   // by convention an object
}

pub type Items = Vec<Item>;

/// Items grouped by port. Most nodes only read/write "main".
pub struct NodeInput  { ports: Vec<(String, Items)> }
pub struct NodeOutput { ports: Vec<(String, Items)> }
```

Every node consumes zero or more items per input port and produces zero or more items per output port. **Per-item nodes** (HTTP, Set, file ops) execute their operation once per input item; zero items in → zero operations → zero items out, which is what makes untaken IF branches free. **Batch nodes** (Merge, Code in all-items mode, Agent) operate on the whole collection once.

### 5.2 The trait

```rust
pub struct PortSpec {
    pub name: &'static str,        // "main", "true", "false", "a", "b", …
    pub label: &'static str,       // UI label
}

/// What a node needs from the permission grants to run at all.
/// The engine unions these across the graph to drive the permissions UI
/// and to refuse runs that lack a grant (§4.6).
#[derive(Default)]
pub struct Needs {
    pub network: bool,
    pub code: bool,
    pub fs_read: bool,
    pub fs_write: bool,
    pub agent: bool,
}

pub struct NodeSpec {
    pub type_name: &'static str,           // "http.request"
    pub type_version: u32,                 // 1
    pub label: &'static str,               // "HTTP Request"
    pub category: &'static str,            // "trigger" | "flow" | "transform" | "action"
    pub inputs: &'static [PortSpec],
    pub outputs: &'static [PortSpec],
    pub params_schema: fn() -> serde_json::Value,  // JSON Schema for the editor + validation
    pub needs: Needs,
    pub is_trigger: bool,
}

pub trait Node: Send + Sync {
    fn spec(&self) -> &'static NodeSpec;
    fn run(&self, ctx: &mut NodeCtx, input: NodeInput) -> Result<NodeOutput, NodeError>;
}
```

The registry is a static map `type_name -> &'static dyn Node`, assembled at startup. Nodes are stateless; all per-run state lives in the context.

### 5.3 `NodeCtx` — everything a node may touch

```rust
impl NodeCtx<'_> {
    // --- parameters ---------------------------------------------------------
    /// Raw params as stored (templates unresolved).
    pub fn params_raw(&self) -> &Value;
    /// Params with every {{ }} template resolved against item `i` of the
    /// node's primary input ("main"). The n8n getNodeParameter(i) equivalent.
    pub fn params(&mut self, i: usize) -> Result<Value, NodeError>;
    /// Params resolved with NO item in scope ($json/$index error if used).
    /// For batch nodes and trigger nodes.
    pub fn params_node(&mut self) -> Result<Value, NodeError>;

    // --- control ------------------------------------------------------------
    /// Cooperative cancellation; long-running nodes MUST poll this between
    /// items and inside internal waits.
    pub fn cancelled(&self) -> bool;
    /// Append a line to the run's .jsonl log + live event stream.
    pub fn log(&mut self, level: LogLevel, msg: &str);

    // --- capability services (each checks the workflow's grants; a denied
    // --- call is a NodeError, so a node can never bypass permissions) -------
    pub fn http(&mut self) -> Result<HttpClient<'_>, NodeError>;   // §8.2
    pub fn fs_read(&mut self, path: &str) -> Result<Vec<u8>, NodeError>;   // §8.3
    pub fn fs_write(&mut self, path: &str, bytes: &[u8], append: bool) -> Result<(), NodeError>;
    pub fn agent(&mut self) -> Result<&dyn AgentRunner, NodeError>; // §8.4
    /// A fresh, sandboxed JS context for the Code node (code permission).
    pub fn js(&mut self) -> Result<JsSandbox<'_>, NodeError>;       // §8.1

    // --- run info -----------------------------------------------------------
    pub fn run_info(&self) -> &RunInfo;   // run id, workflow id/name, trigger, started_at
    /// Output of an earlier node by user-visible name (backs $node(), §6.3).
    pub fn node_output(&self, name: &str) -> Option<&NodeOutput>;
}
```

Design rule: **capability enters only through `NodeCtx`**. A node holds no clients, opens no sockets, touches no paths on its own. Adding a node type never widens the security review surface — the services are the choke point, exactly like `build_grok_args` is for Automations.

### 5.4 Errors

```rust
pub struct NodeError {
    pub kind: ErrorKind,        // Params | Expression | Permission | Http | Fs |
                                // Agent | Code | Timeout | Cancelled | Data | Internal
    pub message: String,        // human-readable, shown in the run UI
    pub item_index: Option<usize>, // which item, when the failure is per-item
}
```

`Permission` errors carry what was needed and what was granted, so the UI can offer a one-click "grant and re-run" flow later.

---

## 6. Expressions

### 6.1 Engine

One QuickJS `Runtime` per workflow run, memory-limited (32 MB) with an interrupt-handler watchdog. Contexts:

- **Expressions:** one `Context` per *node execution*, shared across that node's per-item resolutions. Fresh per node so one node's expression can't pollute another's globals; within a node, pollution is self-sabotage by the same author and accepted. Watchdog: 1 s CPU per evaluation.
- **Code node:** always a fresh `Context`, own timeout (default 5 s, param-overridable), same runtime memory cap.

The sandbox invariant: **contexts get no host bindings** — no file, network, process, or clock-setting APIs, only the injected data globals below. All capability lives in nodes behind `NodeCtx`; an expression can compute, never act. This is what makes it safe for expressions to be always-on while the Code node sits behind a permission.

### 6.2 Template rules

- A string param containing `{{ … }}` is a template; anything else is a literal. No n8n-style `=` prefix.
- If the **entire** string is exactly one `{{ expr }}`, the result keeps the JS value's type — numbers stay numbers, objects stay objects. Otherwise each expression result is stringified and concatenated with the surrounding text.
- Literal braces: `{{ "{{" }}`.
- Templates are resolved **in node params only — never in item data**. A string arriving in an item that happens to contain `{{ … }}` is inert data. This is the injection boundary: untrusted content fetched by HTTP or produced by an agent can never cause evaluation. (The same rule Automations applies to `{{chain}}` upstream text.)

### 6.3 Scope — globals available inside `{{ }}`

| Global | Value |
|---|---|
| `$json` | current item's `json` (per-item resolution only) |
| `$index` | current item index |
| `$items` | all items on the node's primary input |
| `$node("Name")` | earlier node's output: `.all(port = "main")` → items' json array, `.first(port = "main")` → first json or null |
| `$run` | `{ id, workflow: { id, name }, trigger: { kind, reason }, started_at }` |
| `$now` | epoch seconds at evaluation time |
| `$secret("name")` | named secret (§8.5); resolvable only in params the node's spec marks secret-capable (HTTP url/headers/body) |

Deliberately absent: `$env` (would leak the process environment into workflow text) and any require/import mechanism.

---

## 7. Execution model

### 7.1 Lifecycle

```
trigger fires (scheduler / manual / CLI)
  → WorkflowManager admission: enabled? paused? overlap policy? capacity? (queue if full)
  → run created: RunRecord(status=running) written, run thread spawned
      → validate (again), check permissions vs needs
      → seed: fired trigger node executes first, emits its item(s)
      → ready-queue topological execution (§7.3)
      → finalize: RunRecord(status=…) written, state updated, prune, events emitted
```

Admission control mirrors `JobManager` exactly: global pause flag (manual runs exempt), per-workflow overlap `skip | replace`, a capacity cap with a FIFO queue, and a wake channel so config changes re-evaluate immediately. **Cap: 2 concurrent workflow runs**, matching `MAX_CONCURRENT_JOBS`, and the Agent node additionally respects the Automations pool so grok processes never exceed the app-wide budget (open question #3, §12).

### 7.2 Threading

One OS thread per active run; nodes execute **sequentially** within a run. No tokio, no async — consistent with the whole codebase (threads + channels), and n8n itself is effectively sequential per execution. Blocking nodes (Agent minutes-long, Wait) hold their run's thread; with a cap of 2 concurrent runs that's two parked threads worst-case, which is nothing for a desktop app. The ready-queue design (§7.3) is order-agnostic, so intra-run parallelism can be added later without changing semantics — it would only relax "one at a time" to "all ready nodes at once."

### 7.3 Scheduling semantics (the precise rules)

1. **Reachability.** Only nodes reachable by directed edges from the *fired* trigger execute. Other triggers and anything exclusively downstream of them are skipped (recorded as `skipped` in the run).
2. **Readiness.** A node becomes ready when **every connected input port has received a delivery from every edge into it**. Every executed node delivers on every declared output edge exactly once — possibly an empty delivery — so readiness always resolves and empty branches propagate structurally.
3. **Execution.** Ready nodes execute one at a time, ordered deterministically: among ready nodes, lowest node `id` (string order) first. Same graph + same data → same order, always.
4. **Delivery.** Output items go to every edge leaving that port; each target port concatenates deliveries in `connections` order. Unconnected outputs are discarded.
5. **Empty in, empty out.** Per-item nodes with zero total input items perform zero work and emit zero items. An untaken IF branch therefore costs nothing downstream, yet every reachable node still "ran" (with zero items) — there is no special skipped-branch state to reason about, and Merge nodes just work.
6. **Disabled nodes** forward their first input port to their first output port verbatim.
7. Each node executes **exactly once per run**. No cycles (validated), no re-entry.

### 7.4 Trigger nodes

Trigger nodes are ordinary nodes that the *scheduler* (not the graph) causes to fire; in the run itself they execute first and emit the seed item. One workflow may hold several triggers (e.g. a schedule and a manual button); each firing starts an independent run seeded by that trigger.

The workflow scheduler thread ports the proven `jobs.rs` logic per trigger type — interval/daily/weekly wall-clock math with creation-time baseline, git head polling, file snapshot + one-tick settle buffer, `min_interval_secs` rate limit — reading trigger params from trigger nodes and persisting bookkeeping in `state.trigger[<node_id>]`.

Seed items:

| Trigger | Seed `json` |
|---|---|
| `trigger.manual` | `{ "trigger": { "kind": "manual", "fired_at": … } }` |
| `trigger.schedule` | `{ "trigger": { "kind": "schedule", "fired_at": …, "reason": "schedule" \| "schedule-catchup" } }` |
| `trigger.git` | `{ "trigger": { "kind": "git", "commit": "<head>", "prev": "<last seen>", "branch": … } }` |
| `trigger.file` | `{ "trigger": { "kind": "file", "path": …, "glob": … } }` |

### 7.5 Error policy

Per node, `on_error`:

- **`stop`** (default) — the run fails immediately with status `error`, pointing at the node (and item index when per-item). Nothing further executes.
- **`skip`** — the node's outputs become empty on all ports, a warning is recorded, execution continues. (Empty-propagation then naturally no-ops the branch.)
- **`branch`** — the node gains a virtual `error` output port. Per-item failures emit `{ "error": { "kind", "message" }, "item": <input json> }` there while successful items flow out the normal ports; node-level failures emit a single error item. Lets a workflow route failures to a report or notification path.

`retry: { attempts, backoff_secs }` wraps the whole node execution before `on_error` applies (sleep is cancel-aware). Default: no retry.

### 7.6 Cancellation and timeouts

- **Cancel** (user stop, overlap `replace`, app exit): sets the run's `AtomicBool`. The engine checks it between nodes and between items; services check it inside long operations; the Agent node tree-kills its process, exactly like `cancel_run`. Status `cancelled`.
- **Run timeout** (`settings.timeout_secs`): a supervisor watchdog (same waiter-thread pattern as `spawn_run`) flips the same flag and records `timeout` instead.
- **Per-node timeouts** are cooperative and service-level: HTTP has connect/read timeouts, the Code node has its interrupt watchdog, the Agent node has its own `timeout_secs` with a hard kill, Wait polls cancel every 400 ms. There is no thread-kill for a stuck pure-Rust node; the run timeout is the honest backstop.
- **App exit**: `WorkflowManager::cancel_all()` joins the exit path alongside `jobs_exit.cancel_all()` — no orphan processes.

### 7.7 Run records and events

`workflow-runs/<workflow_id>/<run_id>.json`:

```json
{
  "id": "…", "workflow_id": "…",
  "trigger": { "kind": "schedule", "reason": "schedule", "node_id": "n1" },
  "status": "success",
  "started_at": "…", "finished_at": "…",
  "error": { "node_id": "n2", "kind": "http", "message": "…", "item_index": 0 },
  "nodes": [
    { "node_id": "n2", "name": "Summarize commits", "status": "success",
      "items_in": 1, "items_out": 1, "duration_ms": 48211,
      "attempts": 1, "warning": null }
  ],
  "data": { "n2": { "main": { "items": [ … ], "truncated": false } } }
}
```

- `status` vocabulary: `running | success | error | cancelled | timeout | launchfailed` (reusing the Automations words the UI already speaks; `queued` for admission-queued runs).
- Node statuses: `success | error | skipped (unreachable) | cancelled`.
- **`data` capture** obeys `settings.capture`: `sample` (default — first 20 items per port, ≤256 KB per node, `truncated` flagged), `full` (debugging), `none`. Captured params are always the **raw templates**, never resolved values — resolved params may contain secrets and are never persisted (§8.5).
- The `.jsonl` log gets one line per event: run started, node started, node log lines, node finished (with counts), run finished. Append-only via the existing pattern.
- Live events over the `RunEvents` trait — the Tauri adapter re-emits them as `workflow-run-started`, `workflow-node-started`, `workflow-node-finished`, `workflow-run-output`, `workflow-run-finished`, mirroring the `automation-*` family so the frontend event plumbing carries over. Failure while unfocused reuses the taskbar-flash rule.
- Pruning: keep `settings.keep_runs` (default 50), delete meta + log together.

---

## 8. Permissions and threat model

### 8.1 The model

```json
"permissions": {
  "network": { "enabled": false, "hosts": ["api.github.com", "*.roaringbytes.com"], "private_ips": false },
  "code": false,
  "fs": { "read": ["E:/reports"], "write": ["E:/reports/out"] },
  "agent": false
}
```

Default-deny on every axis; a new workflow can transform data and nothing else. The editor computes the union of the graph's `Needs` and shows needed-vs-granted; a run with an uncovered need refuses to start with a per-node message. Grants live in the workflow file — see the threat model below for why that's sound.

Enforcement lives in the `NodeCtx` services (`permissions.rs`), never in node code:

- **`network`** — gates `ctx.http()`. Host allowlist (exact or `*.suffix` wildcard) checked against the URL host, **re-checked on every redirect hop**. Scheme http/https only. Unless `private_ips: true`, resolved addresses in loopback, RFC 1918, link-local, and ULA ranges are refused — and the *resolved* IP is pinned for the actual connection (custom resolver), closing the DNS-rebinding gap between check and connect. `private_ips: true` is a legitimate, explicit opt-in (homelab targets like BytesPanel or local services are a first-class use case) that the UI badges loudly. Response cap 10 MB, connect/read timeouts 30 s, redirects ≤ 5.
- **`code`** — gates `ctx.js()`, i.e. the Code node. Expressions are *not* gated: they run in the same no-capability sandbox, so they can compute but not act (§6.1).
- **`fs`** — gates `ctx.fs_read/fs_write`. Path-prefix allowlists compared on **canonicalized** paths (junctions and symlinks resolved — the audit A4 lesson applied from day one); for writes to not-yet-existing files the parent directory is canonicalized. No traversal out of a granted prefix.
- **`agent`** — gates `ctx.agent()`. Agent runs are **always effective-shadow**: the injected runner routes through the same arg-builder discipline as `jobs.rs` (read-safe tool intersection, `--tools` always emitted, baked deny list, no subagents, stdin null, tree-kill). The write-mode gate stays where it is today — a workflow cannot reach a capability Automations doesn't have.

### 8.2–8.4 (service details folded into §8.1 above)

### 8.5 Secrets

v1: `~/.swervebuild/secrets.json`, a flat `{ "name": "value" }` map managed in Settings, read via the injected `SecretStore`. Honest about its level: plaintext at rest in the user's profile, same trust domain as everything else in `~/.swervebuild` (including grok's own session files). Referenced as `{{ $secret("github_pat") }}` — only in params marked secret-capable (HTTP url/headers/body), so a secret can't be quietly routed into an agent prompt or a written file by template. Not persisted: run captures store raw templates only. Upgrade path (post-v1): swap the `SecretStore` impl for a Windows Credential Manager (`keyring`) backend — the trait seam exists for exactly this, alongside the app's endpoint-API-key migration.

### 8.6 Threat model, stated plainly

- **In scope:** a curious/careless workflow author; malicious or malformed *data* flowing through nodes (hostile HTTP responses, hostile repo content summarized by the agent); a hand-edited workflow file; SSRF from the HTTP node toward LAN services; runaway resource use.
- **Structural guarantees (not liftable from JSON):** expressions/Code have no host bindings; capability flows only through permission-checked `NodeCtx` services; agent runs are effective-shadow with the read-safe intersection in Rust; scheme/size/time caps and redirect+rebinding checks on HTTP; canonicalized path prefixes on fs; no shell node exists; item data is never template-evaluated.
- **Out of scope:** an attacker with write access to `~/.swervebuild` (they own the profile and could equally edit `automations.json` or the app config — permission grants in the workflow file add nothing new to that attacker); protecting secrets from local malware (v1 plaintext store, stated above).

---

## 9. v1 node catalog

Small on purpose — the engine milestone needs breadth of *kinds* (trigger, flow, transform, action, code, agent), not a big library. Adding a node later is one file + a registry line.

| Type | Ports in → out | Needs | Notes |
|---|---|---|---|
| `trigger.manual` | — → main | — | Run-now button / CLI |
| `trigger.schedule` | — → main | — | interval / daily / weekly, ported math |
| `trigger.git` | — → main | — | head polling, per-node bookkeeping |
| `trigger.file` | — → main | — | snapshot + settle, ported |
| `http.request` | main → main | network | method, url, query, headers, body (json/text/form); response item `{ status, headers, body }` (json-parsed when content-type says so); executes once per input item |
| `transform.set` | main → main | — | ops list: set / rename / remove / keep, dotted paths, values template-capable; per item |
| `flow.if` | main → true, false | — | condition list (`eq ne gt gte lt lte contains matches exists`), combine and/or; routes each item |
| `flow.merge` | a, b → main | — | modes: `append` (a then b) \| `zip` (pair by index into `{a, b}`) |
| `code.js` | main → main | code | modes: `all_items` (gets `items`, returns array) \| `per_item` (gets `item`, returns item/null); `console.log` → run log |
| `agent.run` | main → main | agent | prompt (template-capable), cwd, model/effort/max_turns/timeout/web_search/json_schema; batch node — one agent turn per *execution*, not per item; output `{ text, structured, stop_reason, session_id }` |
| `file.read` | main → main | fs.read | utf-8 text v1; per item |
| `file.write` | main → main | fs.write | path + content templates, `overwrite` \| `append`; passes input through |
| `util.wait` | main → main | — | seconds; cancel-aware sleep; passes input through |

Deferred (v2+, in rough order of pull): `flow.switch`, split/aggregate items, dedupe, Windows-toast notify node, webhook trigger, sub-workflow, Loop, binary items, an LLM-API sugar node (today that's just `http.request`).

---

## 10. Coexistence with Automations

- Nothing in `jobs.rs`, `automations.json`, or the Automations UI changes. Separate store, separate runs directory, separate scheduler thread, separate manager, separate nav section ("Workflows").
- Id prefixes (`a-` / `w-`) keep every surface unambiguous.
- Conceptually an automation is the degenerate workflow `trigger → agent.run`; the eventual convergence (a convert-to-workflow button producing that two-node graph, recipes, and a deprecation decision) is **M4, decided only after the engine has earned it**. No forced migration ever — the shipped feature keeps working.

---

## 11. Risks, with mitigations

| Risk | Mitigation |
|---|---|
| **`rquickjs` bundled build on Windows/MSVC** — historically the sharpest edge of embedding QuickJS | **M0 task #1 is a spike**: compile + eval + memory-limit + interrupt test in CI's Windows job before anything else. `expr.rs` fronts the engine behind a small internal trait, so the fallback — `boa_engine`, pure Rust, slower but zero C toolchain risk — is a swap, not a rewrite |
| DNS rebinding / SSRF nuance in the HTTP service | resolver-pinning + per-hop re-validation specified up front (§8.1); dedicated tests with a local redirector in M1 |
| Item-set memory blowup | `max_items_per_port` (10 k), 10 MB response cap, capture sampling; hard errors, not silent truncation of live data |
| A stuck pure-Rust node can't be force-killed | run-level supervisor timeout is the backstop; every service call is internally time-bounded, so the exposure is a bug in a built-in node, caught in tests |
| Thread-per-run + blocking Wait/Agent | acceptable at cap 2; §7.2 keeps the door open for intra-run parallelism without semantic change |
| Scope creep toward the editor before the engine is proven | the M-gates below; M3 (canvas) is not started until the M2 gate passes |

---

## 12. Open questions for the owner

1. **Secrets v1** — is the plaintext `secrets.json` (§8.5) acceptable to start, with the keyring swap scheduled later alongside the endpoint-API-key migration? The alternative is doing keyring first and delaying the HTTP node's usefulness.
2. **Run-data capture default** — `sample` (proposed) keeps records small but means the run view shows the first 20 items per port unless a workflow opts into `full`. Comfortable?
3. **Process budget** — should Agent-node runs share the Automations `MAX_CONCURRENT_JOBS = 2` grok budget (proposed: yes, one global budget), or get their own pool?
4. **`flow.merge` zip mode** — needed in v1, or ship `append` only and add `zip` when a real workflow wants it?

Everything else in this doc I'm treating as decided unless you push back on it.

---

## 13. Milestones (engine-first, each with a hard gate)

**M0 — headless engine skeleton.** Workspace crate; QuickJS spike (risk #1) first; model + validation + executor + expressions; nodes: `trigger.manual`, `transform.set`, `flow.if`, `flow.merge`; `swervebuild_workflow` CLI; integration tests with golden run records.
*Gate:* `swervebuild-workflow run examples/hello.json` executes a branching workflow from JSON with zero UI; `cargo test` green on Windows CI.

**M1 — capabilities.** `permissions.rs` services + `http.request`, `code.js`, `file.read/write`, `util.wait`; SSRF/redirect/rebinding tests against a local test server; permission-refusal tests; secrets store + `$secret`.
*Gate:* a workflow calls a local HTTP endpoint under an allowlist, and every documented denial path has a test proving it denies.

**M2 — app integration.** `AgentRunner` impl wrapping the jobs.rs invocation discipline; `agent.run`; schedule/git/file trigger nodes + workflow scheduler thread; `WorkflowManager` admission; run records + pruning; Tauri commands + `workflow-*` events; a minimal list-and-runs UI (no canvas).
*Gate:* the nightly-digest example (§4.1) runs unattended from a schedule inside the app, its run inspectable in the UI, with Automations demonstrably untouched (existing e2e still green).

**M3 — editor.** Svelte Flow canvas, palette, param inspector, permissions editor, run overlay on the graph. Gets its own design doc; not started before the M2 gate.

**M4 — convergence.** Convert-automation-to-workflow, recipes, docs, and the subsume-or-coexist decision — made with real usage data, not up front.

---

## 14. As built (2026-07-20) — deviations and resolutions

M0 through M3 shipped in one pass. Verification: engine crate 37 tests, app crate 29 tests, `svelte-check` 0/0, `npm run build` green, e2e 20/20 (live grok leg included), CLI gate passed (`swervebuild-workflow run crates/swerve-workflows/examples/hello.json`), canvas verified in a browser (palette add, select, inspector, edge drag, run overlay plumbing).

**§12 open questions, resolved in code:**
1. Secrets v1 = plaintext `~/.swervebuild/secrets.json` behind the `SecretStore` trait; managed from the Workflows page (Secrets dialog). Keyring swap remains the upgrade path.
2. Run-data capture defaults to `sample`; per-workflow setting in the editor (right panel, nothing selected).
3. **Deviation:** workflow runs get their OWN cap (2 concurrent, `workflows_tauri.rs`) and Agent nodes spawn grok directly rather than through the Automations `JobManager` — worst case 2 automation + 2 workflow grok processes at once. Revisit if that ever bites.
4. Merge shipped with both `append` and `zip`.

**Spec deviations worth knowing:**
- **Readiness nuance:** edges from nodes not reachable in the current run deliver nothing and are excluded from readiness counting (else a Merge fed by two triggers could deadlock). §7.3 as written implied all connected edges.
- **Run timeout** is enforced by the engine itself between items/nodes plus per-service internal bounds (HTTP timeouts, Code watchdog, Agent kill) — no separate supervisor thread. Observable behavior matches §7.6.
- **`$secret()`** is whole-node capability (any param of a `secrets_ok` node, i.e. HTTP), not per-param as §8.5 sketched.
- **Code node's `code` param is never template-resolved** — JS source with `{{` stays literal.
- `TriggerFire` gained an optional caller-assigned `run_id` so the manager can track queued runs.
- Trigger params for git/file live on the trigger node (`cwd`/`branch`, `path`/`glob`); bookkeeping in `state.trigger[<node_id>]` as designed.
- The CLI runs with the REAL grok agent runner and secrets (faithful rehearsal), not a stub.
- Agent node rejects local (`local:`) models for now — the llama-server lifecycle needs an AppHandle the runner does not have.
- Param forms are a TS-side registry (`src/lib/workflows/model.ts`) mirroring the Rust specs; ports/needs/labels still come from the Rust catalog command.

**Left for later:** editor niceties (copy/paste, undo, drag-from-palette), webhook trigger, loops, binary items, sub-workflows, keyring secrets, M4 convergence.

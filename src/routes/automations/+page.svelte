<script lang="ts">
  import { onMount } from "svelte";
  import { subscribe } from "$lib/events";
  import { workspaceStore } from "$lib/stores/workspace.svelte";
  import { automationsStore } from "$lib/stores/automations.svelte";
  import { providerStore } from "$lib/stores/providers.svelte";
  import Icon from "$lib/components/ui/Icon.svelte";
  import EmptyState from "$lib/components/ui/EmptyState.svelte";
  import StatusPill from "$lib/components/ui/StatusPill.svelte";
  import RunMap from "$lib/components/automations/RunMap.svelte";
  import { invoke } from "@tauri-apps/api/core";
  import type { Automation, ModelInfo, RunRecord, RunFinished, RunOutput } from "$lib/types";
  import {
    newAutomation,
    recipes,
    triggerIcon,
    triggerSummary,
    statusTone,
    statusLabel,
    friendlyError,
    currentTzOffset,
  } from "$lib/automations/model";

  type TranscriptLine = { kind: "thought" | "text"; text: string };

  let loading = $state(true);
  let view = $state<"list" | "map">("list");
  let showRecipes = $state(false);
  let busy = $state<string | null>(null);

  // editor
  let editing = $state<Automation | null>(null);
  let editorError = $state<string | null>(null);
  let advanced = $state(false);
  let modelOptions = $state<ModelInfo[]>([]);

  // run history (per automation id)
  let expanded = $state<Record<string, boolean>>({});
  let runsByAutomation = $state<Record<string, RunRecord[]>>({});

  // inspector
  let inspector = $state<{ automationId: string; runId: string; name: string } | null>(null);
  let transcript = $state<TranscriptLine[]>([]);
  let inspectorStatus = $state<RunRecord["status"] | "running">("running");
  let inspectorError = $state<string | null>(null);
  let showRawError = $state(false);

  const automations = $derived(automationsStore.automations);
  const paused = $derived(automationsStore.paused);
  const projects = $derived(workspaceStore.projects);
  const runningCount = $derived(
    automations.filter((a) => a.state.last_status === "running").length,
  );
  const enabledCount = $derived(automations.filter((a) => a.enabled).length);

  // P2.1: automations are grok-only by design (jobs.rs spawns the Grok CLI).
  // When the chat provider is something else, say so instead of letting the
  // user assume their active agent runs the rules.
  const grokRow = $derived(providerStore.all.find((p) => p.id === "grok"));
  const nonGrokActive = $derived(providerStore.active.id !== "grok");

  onMount(() => {
    (async () => {
      await workspaceStore.refresh();
      await providerStore.load();
      await automationsStore.refresh();
      loading = false;
      try {
        modelOptions = await invoke<ModelInfo[]>("list_models");
      } catch {
        modelOptions = [];
      }
    })();

    const offs = [
      subscribe<RunOutput>("automation-run-output", (e) => {
        if (!inspector || e.payload.runId !== inspector.runId) return;
        const line: TranscriptLine = { kind: e.payload.type, text: e.payload.text };
        if (line.kind === "text") {
          const last = transcript[transcript.length - 1];
          if (last && last.kind === "text") {
            last.text += line.text;
            transcript = [...transcript];
            return;
          }
        }
        transcript = [...transcript, line];
      }),

      subscribe<{ automationId: string; runId: string }>("automation-run-started", () => {
        refreshSoon();
      }),

      subscribe<RunFinished>("automation-run-finished", (e) => {
        refreshSoon();
        loadRuns(e.payload.automationId);
        if (inspector && e.payload.runId === inspector.runId) {
          inspectorStatus = e.payload.status;
          inspectorError = e.payload.error;
        }
      }),
    ];

    return () => {
      offs.forEach((o) => o());
      if (refreshTimer) clearTimeout(refreshTimer);
    };
  });

  async function loadRuns(automationId: string) {
    runsByAutomation[automationId] = await automationsStore.runs(automationId);
  }

  // Runs commonly start/finish together, and each refresh is several invokes.
  // Coalesce them instead of issuing a burst per event.
  let refreshTimer: ReturnType<typeof setTimeout> | null = null;
  function refreshSoon() {
    if (refreshTimer) clearTimeout(refreshTimer);
    refreshTimer = setTimeout(() => {
      refreshTimer = null;
      automationsStore.refresh();
    }, 150);
  }

  function openNew() {
    const p = projects[0];
    editing = newAutomation(p?.path ?? "", p?.id ?? null);
    editorError = null;
    advanced = false;
    showRecipes = false;
  }

  function openRecipe(recipeId: string) {
    const p = projects[0];
    const recipe = recipes.find((r) => r.id === recipeId);
    if (!recipe) return;
    editing = recipe.build(p?.path ?? "", p?.id ?? null);
    editorError = null;
    advanced = false;
    showRecipes = false;
  }

  function openEdit(a: Automation) {
    editing = structuredClone($state.snapshot(a)) as Automation;
    editorError = null;
    advanced = false;
  }

  function onProjectChange(projectId: string) {
    if (!editing) return;
    const project = projects.find((p) => p.id === projectId);
    editing.project_id = projectId || null;
    editing.executor.cwd = project ? project.path : "";
    // A file trigger's watch path was seeded from the previous project's folder;
    // repoint it, or it silently keeps watching the old project.
    if (editing.trigger.kind === "file") {
      editing.trigger.path = editing.executor.cwd;
    }
    editorError = null;
  }

  function setTriggerKind(kind: string) {
    if (!editing) return;
    if (kind === "manual") editing.trigger = { kind: "manual" };
    else if (kind === "schedule")
      editing.trigger = {
        kind: "schedule",
        every: "daily",
        interval_minutes: 60,
        hour: 9,
        minute: 0,
        weekday: 1,
        tz_offset_minutes: currentTzOffset(),
      };
    else if (kind === "git")
      editing.trigger = { kind: "git", branch: "main", last_seen_commit: null };
    else if (kind === "file")
      editing.trigger = { kind: "file", path: editing.executor.cwd, glob: "*.md", snapshot: null };
  }

  async function save() {
    if (!editing) return;
    if (!editing.executor.cwd.trim()) {
      editorError = "Choose a project — an automation needs a folder to run in.";
      return;
    }
    editorError = null;
    if (!editing.name.trim()) {
      editing.name = "Untitled automation";
    }
    // Re-stamp the timezone offset for schedule triggers at save time.
    if (editing.trigger.kind === "schedule") {
      editing.trigger.tz_offset_minutes = currentTzOffset();
    }
    busy = "save";
    try {
      await automationsStore.save($state.snapshot(editing) as Automation);
      editing = null;
    } catch (err) {
      // Previously uncaught: a failed save left the editor open with no feedback.
      editorError = `Couldn't save this automation: ${String(err)}`;
    } finally {
      busy = null;
    }
  }

  async function del(a: Automation) {
    if (!confirm(`Delete "${a.name}"? Its run history is kept.`)) return;
    await automationsStore.remove(a.id);
  }

  async function runNow(a: Automation) {
    if (!a.executor.cwd.trim()) {
      inspector = { automationId: a.id, runId: "", name: a.name };
      inspectorStatus = "launchfailed";
      inspectorError = "No project folder is set. Edit this automation and choose a project.";
      transcript = [];
      return;
    }
    busy = a.id;
    transcript = [];
    inspectorStatus = "running";
    inspectorError = null;
    showRawError = false;
    try {
      const runId = await automationsStore.runNow(a.id);
      inspector = { automationId: a.id, runId, name: a.name };
    } catch (err) {
      inspector = { automationId: a.id, runId: "", name: a.name };
      inspectorStatus = "launchfailed";
      inspectorError = String(err);
    } finally {
      busy = null;
    }
  }

  async function toggleExpand(a: Automation) {
    expanded[a.id] = !expanded[a.id];
    if (expanded[a.id] && !runsByAutomation[a.id]) {
      await loadRuns(a.id);
    }
  }

  async function openInspector(a: Automation, run: RunRecord) {
    inspector = { automationId: a.id, runId: run.id, name: a.name };
    inspectorStatus = run.status;
    inspectorError = run.error;
    showRawError = false;
    transcript = [];
    const raw = await automationsStore.log(a.id, run.id);
    const lines: TranscriptLine[] = [];
    for (const l of raw.split("\n")) {
      if (!l.trim()) continue;
      try {
        const v = JSON.parse(l);
        if (v.type === "text") lines.push({ kind: "text", text: v.text ?? "" });
        else if (v.type === "thought") lines.push({ kind: "thought", text: v.text ?? "" });
      } catch {
        /* skip non-JSON */
      }
    }
    transcript = lines;
    // Mark this run seen (clears the failure badge).
    if (!run.seen) await automationsStore.markSeen(a.id, [run.id]);
  }

  function closeInspector() {
    inspector = null;
    transcript = [];
  }

  async function togglePause() {
    await automationsStore.setPaused(!paused);
  }

  function lastRunText(a: Automation): string {
    if (!a.state.last_fired_at) return "Never run";
    const secs = Math.floor(Date.now() / 1000 - a.state.last_fired_at);
    if (secs < 60) return "Last run just now";
    if (secs < 3600) return `Last run ${Math.floor(secs / 60)}m ago`;
    if (secs < 86400) return `Last run ${Math.floor(secs / 3600)}h ago`;
    return `Last run ${Math.floor(secs / 86400)}d ago`;
  }

  function isSilent(a: Automation): boolean {
    return a.state.last_status === "success";
  }

  // The folder an automation runs in: its project's name, or the raw path.
  function folderLabel(a: Automation): string {
    const p = projects.find((x) => x.id === a.project_id);
    return p ? p.name : a.executor.cwd;
  }
  function hasFolder(a: Automation): boolean {
    return !!a.executor.cwd.trim();
  }
</script>

<svelte:window
  onkeydown={(e) => {
    if (e.key === "Escape") {
      if (inspector) closeInspector();
      else if (editing) editing = null;
    } else if (e.key === "n" && !editing && !inspector && document.activeElement === document.body) {
      openNew();
    }
  }}
/>

<div class="page">
  <header class="page-header">
    <div class="header-row">
      <div>
        <h1 class="page-title">Automations</h1>
        <p class="page-subtitle">
          Run Grok headless on a trigger — read-only by default.
          {#if enabledCount > 0}· {enabledCount} enabled{/if}
          {#if runningCount > 0}· {runningCount} running{/if}
        </p>
        {#if grokRow && !grokRow.available}
          <p class="page-subtitle provider-note" data-testid="automations-grok-note">
            Grok Build isn't installed, so automations can't run — install it from
            the home screen first.
          </p>
        {:else if nonGrokActive}
          <p class="page-subtitle provider-note" data-testid="automations-grok-note">
            Automations always run on Grok Build — your active chat provider
            ({providerStore.active.label}) doesn't apply here.
          </p>
        {/if}
      </div>
      <div class="header-actions">
        {#if automations.length > 0}
          <div class="segmented" role="tablist" aria-label="View">
            <button class:active={view === "list"} onclick={() => (view = "list")} type="button">
              <Icon name="list" size={14} /> List
            </button>
            <button class:active={view === "map"} onclick={() => (view = "map")} type="button">
              <Icon name="map" size={14} /> Map
            </button>
          </div>
          <button class="btn btn-ghost" type="button" onclick={togglePause}>
            <Icon name={paused ? "play" : "pause"} size={14} />
            {paused ? "Resume" : "Pause all"}
          </button>
        {/if}
        <button class="btn btn-primary" type="button" onclick={openNew}>
          <Icon name="plus" size={15} /> New automation
        </button>
      </div>
    </div>
    {#if paused}
      <div class="notice warning">
        All automations are paused — triggers won't fire. <strong>Run now</strong> still works.
      </div>
    {/if}
  </header>

  {#if editing}
    <!-- Editor -->
    <section class="card editor">
      <div class="editor-head">
        <h2>{editing.id ? "Edit automation" : "New automation"}</h2>
        <span class="shadow-note">
          <StatusPill tone="accent" label="Shadow mode — read-only" />
        </span>
      </div>

      <label class="field">
        <span class="field-label">Name</span>
        <input class="input" bind:value={editing.name} placeholder="Nightly health check" />
      </label>

      <label class="field">
        <span class="field-label">Project — the folder the agent runs in</span>
        <select
          class="input"
          value={editing.project_id ?? ""}
          onchange={(e) => onProjectChange((e.target as HTMLSelectElement).value)}
        >
          <option value="">Choose a project…</option>
          {#each projects as p (p.id)}
            <option value={p.id}>{p.name}</option>
          {/each}
        </select>
        {#if editing.executor.cwd}
          <span class="field-hint">Runs in <code>{editing.executor.cwd}</code></span>
        {:else if projects.length === 0}
          <span class="field-hint warn">No projects yet — <a href="/projects">add a project folder</a> first.</span>
        {:else}
          <span class="field-hint warn">Pick a project — an automation needs a folder to run in.</span>
        {/if}
      </label>

      <div class="field">
        <span class="field-label">Trigger</span>
        <div class="trigger-picker">
          {#each [{ k: "manual", l: "On demand", i: "play" }, { k: "schedule", l: "Schedule", i: "clock" }, { k: "git", l: "Git commit", i: "git-branch" }, { k: "file", l: "File change", i: "file" }] as opt}
            <button
              type="button"
              class="trigger-opt"
              class:active={editing.trigger.kind === opt.k}
              onclick={() => setTriggerKind(opt.k)}
            >
              <Icon name={opt.i as any} size={15} />
              {opt.l}
            </button>
          {/each}
        </div>
      </div>

      {#if editing.trigger.kind === "schedule"}
        <div class="row-fields">
          <label class="field">
            <span class="field-label">Frequency</span>
            <select class="input" bind:value={editing.trigger.every}>
              <option value="daily">Every day</option>
              <option value="weekly">Every week</option>
              <option value="interval">Every N minutes</option>
            </select>
          </label>
          {#if editing.trigger.every === "interval"}
            <label class="field">
              <span class="field-label">Minutes (min 15)</span>
              <input class="input" type="number" min="15" bind:value={editing.trigger.interval_minutes} />
            </label>
          {:else}
            {#if editing.trigger.every === "weekly"}
              <label class="field">
                <span class="field-label">Day</span>
                <select class="input" bind:value={editing.trigger.weekday}>
                  {#each ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"] as d, i}
                    <option value={i}>{d}</option>
                  {/each}
                </select>
              </label>
            {/if}
            <label class="field">
              <span class="field-label">Time</span>
              <input
                class="input"
                type="time"
                value={`${String(editing.trigger.hour).padStart(2, "0")}:${String(editing.trigger.minute).padStart(2, "0")}`}
                onchange={(e) => {
                  if (editing?.trigger.kind !== "schedule") return;
                  const [h, m] = (e.target as HTMLInputElement).value.split(":");
                  editing.trigger.hour = Number(h);
                  editing.trigger.minute = Number(m);
                }}
              />
            </label>
          {/if}
        </div>
      {:else if editing.trigger.kind === "git"}
        <label class="field">
          <span class="field-label">Branch</span>
          <input class="input" bind:value={editing.trigger.branch} placeholder="main" />
        </label>
      {:else if editing.trigger.kind === "file"}
        <div class="row-fields">
          <label class="field">
            <span class="field-label">Watch folder</span>
            <input class="input" bind:value={editing.trigger.path} placeholder="E:\\AgentInbox" />
          </label>
          <label class="field">
            <span class="field-label">Match</span>
            <input class="input" bind:value={editing.trigger.glob} placeholder="*.md" />
          </label>
        </div>
      {/if}

      <label class="field">
        <span class="field-label">Prompt</span>
        <textarea class="input" rows="4" bind:value={editing.executor.prompt} placeholder="What should the agent do? Reply with exactly SILENT if there's nothing to report."></textarea>
      </label>

      <button class="disclosure" type="button" onclick={() => (advanced = !advanced)}>
        <span class="chevron" class:open={advanced}><Icon name="chevron-right" size={14} /></span>
        Advanced
      </button>
      {#if advanced}
        <div class="advanced">
          <label class="field row">
            <span class="field-label">Max turns</span>
            <input class="input narrow" type="number" min="1" bind:value={editing.executor.max_turns} />
          </label>
          <label class="field row">
            <span class="field-label">Timeout (seconds)</span>
            <input class="input narrow" type="number" min="30" bind:value={editing.executor.timeout_secs} />
          </label>
          <label class="field row">
            <span class="field-label">Reasoning effort</span>
            <select class="input narrow" bind:value={editing.executor.effort}>
              <option value={null}>Default</option>
              <option value="low">Low</option>
              <option value="medium">Medium</option>
              <option value="high">High</option>
            </select>
          </label>
          <label class="field row">
            <span class="field-label">Model</span>
            <select class="input narrow" bind:value={editing.executor.model}>
              <option value={null}>Default</option>
              {#each modelOptions as m (m.id)}
                <option value={m.id}>{m.kind === "endpoint" ? "Custom endpoint" : m.label}</option>
              {/each}
            </select>
          </label>
          <label class="field row">
            <span class="field-label">Run after</span>
            <select
              class="input narrow"
              value={editing.chain_input?.from ?? ""}
              onchange={(e) => {
                if (!editing) return;
                const v = (e.target as HTMLSelectElement).value;
                editing.chain_input = v ? { from: v } : null;
              }}
            >
              <option value="">Nothing</option>
              {#each automations.filter((x) => x.id && x.id !== editing?.id) as other (other.id)}
                <option value={other.id}>{other.name}</option>
              {/each}
            </select>
          </label>
          {#if editing.chain_input}
            <p class="field-hint">Its latest result is available in your prompt as <code>{`{{chain}}`}</code>.</p>
          {/if}
          <label class="field row">
            <span class="field-label">
              Allow writes
              <span class="badge badge-muted">Later build</span>
            </span>
            <input type="checkbox" disabled title="Write mode unlocks in a later build. Shadow mode is read-only." />
          </label>
          <p class="field-hint">
            Shadow mode runs with read-only tools (read, grep, list, and read-only shell commands),
            enforced in the app itself — the agent can look but not change files.
          </p>
        </div>
      {/if}

      {#if editorError}
        <p class="editor-error">{editorError}</p>
      {/if}
      <div class="editor-actions">
        <button class="btn btn-ghost" type="button" onclick={() => (editing = null)}>Cancel</button>
        <button class="btn btn-primary" type="button" disabled={busy === "save"} onclick={save}>
          {busy === "save" ? "Saving…" : "Save automation"}
        </button>
      </div>
    </section>
  {/if}

  {#if loading}
    <p class="muted">Loading…</p>
  {:else if automations.length === 0 && !editing}
    <section class="card">
      <EmptyState
        icon="zap"
        title="No automations yet"
        description="Automations run Grok headless on a trigger — a schedule, a git change, a file change, or on demand. New automations start in read-only shadow mode, so they can look but never touch your files."
      >
        <button class="btn btn-primary" type="button" onclick={openNew}>
          <Icon name="plus" size={15} /> New automation
        </button>
        <button class="btn btn-ghost" type="button" onclick={() => (showRecipes = !showRecipes)}>
          Start from a recipe
        </button>
      </EmptyState>
    </section>
  {/if}

  {#if showRecipes}
    <div class="recipe-grid">
      {#each recipes as r (r.id)}
        <button class="tile" type="button" onclick={() => openRecipe(r.id)}>
          <span class="tile-icon"><Icon name={r.icon} size={18} /></span>
          <span class="tile-name">{r.name}</span>
          <span class="tile-blurb">{r.blurb}</span>
        </button>
      {/each}
    </div>
  {/if}

  {#if !loading && automations.length > 0}
    {#if view === "map"}
      <RunMap {automations} {projects} onopen={(a) => openEdit(a)} />
    {:else}
      <div class="auto-list">
        {#each automations as a (a.id)}
          {@const runs = runsByAutomation[a.id] ?? []}
          <section class="card auto-card">
            <div class="auto-head">
              <div class="auto-main">
                <span class="trig-icon"><Icon name={triggerIcon(a.trigger)} size={16} /></span>
                <div class="auto-meta">
                  <div class="auto-name-row">
                    <h2>{a.name}</h2>
                    {#if a.executor.mode === "shadow"}
                      <StatusPill tone="accent" label="Shadow" />
                    {/if}
                    {#if a.state.last_status}
                      <StatusPill
                        tone={statusTone(a.state.last_status as any)}
                        label={statusLabel(a.state.last_status as any, isSilent(a))}
                        pulse={a.state.last_status === "running"}
                      />
                    {/if}
                  </div>
                  <p class="auto-sub">
                    {#if hasFolder(a)}
                      <span class="folder" title={a.executor.cwd}><Icon name="folder" size={11} /> {folderLabel(a)}</span>
                    {:else}
                      <span class="folder warn"><Icon name="folder" size={11} /> No project — won't run</span>
                    {/if}
                    · {triggerSummary(a.trigger)} · {lastRunText(a)}
                  </p>
                </div>
              </div>
              <div class="auto-actions">
                <button class="btn btn-accent btn-sm" type="button" disabled={busy === a.id} onclick={() => runNow(a)}>
                  <Icon name="play" size={13} /> Run now
                </button>
                <button class="btn btn-icon" type="button" title="Edit" onclick={() => openEdit(a)}>
                  <Icon name="settings" size={15} />
                </button>
                <button class="btn btn-icon" type="button" title="Delete" onclick={() => del(a)}>
                  <Icon name="trash" size={15} />
                </button>
              </div>
            </div>

            <button class="runs-toggle" type="button" onclick={() => toggleExpand(a)}>
              <span class="chevron" class:open={expanded[a.id]}><Icon name="chevron-right" size={13} /></span>
              Run history
            </button>

            {#if expanded[a.id]}
              <div class="runs">
                {#if runs.length === 0}
                  <p class="muted small">No runs yet. Hit <strong>Run now</strong> to try it.</p>
                {:else}
                  {#each runs.slice(0, 12) as run (run.id)}
                    <button class="run-row" type="button" onclick={() => openInspector(a, run)}>
                      <StatusPill
                        tone={statusTone(run.status)}
                        label={statusLabel(run.status, run.final_text === "SILENT")}
                        pulse={run.status === "running"}
                      />
                      <span class="run-reason">{run.trigger_reason}</span>
                      <span class="run-time">{new Date(Number(run.started_at) * 1000).toLocaleString()}</span>
                    </button>
                  {/each}
                {/if}
              </div>
            {/if}
          </section>
        {/each}
      </div>
    {/if}
  {/if}
</div>

{#if inspector}
  <div class="overlay" role="dialog" aria-modal="true" tabindex="-1">
    <div class="inspector">
      <header class="insp-head">
        <div class="insp-title">
          <StatusPill
            tone={inspectorStatus === "running" ? "accent" : statusTone(inspectorStatus)}
            label={statusLabel(inspectorStatus === "running" ? "running" : inspectorStatus)}
            pulse={inspectorStatus === "running"}
          />
          <h2>{inspector.name}</h2>
        </div>
        <button class="btn btn-icon" type="button" onclick={closeInspector} title="Close">
          <Icon name="close" size={16} />
        </button>
      </header>

      {#if inspectorError && inspectorStatus !== "running" && inspectorStatus !== "success"}
        <div class="notice danger">
          {friendlyError(inspectorStatus, inspectorError)}
          <button class="link" type="button" onclick={() => (showRawError = !showRawError)}>
            {showRawError ? "Hide details" : "Details"}
          </button>
          {#if showRawError}
            <pre class="raw-err">{inspectorError}</pre>
          {/if}
        </div>
      {/if}

      <div class="transcript">
        {#if transcript.length === 0}
          <p class="muted small">
            {inspectorStatus === "running" ? "Waiting for output…" : "No transcript."}
          </p>
        {:else}
          {#each transcript as line, i (i)}
            {#if line.kind === "thought"}
              <div class="t-thought">{line.text}</div>
            {:else}
              <div class="t-text">{line.text}</div>
            {/if}
          {/each}
        {/if}
      </div>
    </div>
  </div>
{/if}

<style>
  .page {
    max-width: 860px;
    margin-inline: auto;
  }
  .page-header {
    margin-bottom: 1.25rem;
  }
  /* P2.1: grok-only honesty line under the subtitle. */
  .provider-note {
    margin-top: 0.3rem;
    color: var(--accent, #d97757);
    opacity: 0.85;
  }
  .header-row {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 1rem;
  }
  .header-actions {
    display: flex;
    gap: 0.5rem;
    align-items: center;
    flex-wrap: wrap;
    justify-content: flex-end;
  }

  .segmented {
    display: inline-flex;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    overflow: hidden;
  }
  .segmented button {
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
    padding: 0.4rem 0.65rem;
    background: var(--bg-surface);
    border: none;
    color: var(--text-secondary);
    font-size: 0.8125rem;
    cursor: pointer;
  }
  .segmented button.active {
    background: var(--bg-active);
    color: var(--text-primary);
  }
  .segmented button + button {
    border-left: 1px solid var(--border);
  }

  .notice {
    margin-top: 0.75rem;
    padding: 0.6rem 0.85rem;
    border-radius: var(--radius-sm);
    font-size: 0.8125rem;
  }
  .notice.warning {
    background: var(--warning-tint);
    color: var(--warning);
  }
  .notice.danger {
    background: var(--danger-tint);
    color: var(--danger);
  }

  .muted {
    color: var(--text-muted);
  }
  .small {
    font-size: 0.8125rem;
  }

  /* editor */
  .editor {
    padding: 1.1rem 1.15rem;
    margin-bottom: 1.25rem;
    display: flex;
    flex-direction: column;
    gap: 0.9rem;
  }
  .editor-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 1rem;
  }
  .editor-head h2 {
    font-size: 1rem;
    font-weight: 600;
  }
  .field {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
  }
  .field.row {
    flex-direction: row;
    align-items: center;
    justify-content: space-between;
  }
  .field-label {
    font-size: 0.875rem;
    font-weight: 500;
    color: var(--text-primary);
    display: inline-flex;
    align-items: center;
    gap: 0.5rem;
  }
  .field-hint {
    font-size: 0.75rem;
    color: var(--text-muted);
  }
  .field-hint.warn {
    color: var(--warning);
  }
  .field-hint code {
    font-family: var(--font-mono);
    font-size: 0.7rem;
    color: var(--text-secondary);
  }
  .editor-error {
    font-size: 0.8125rem;
    color: var(--danger);
    background: var(--danger-tint);
    border-radius: var(--radius-sm);
    padding: 0.5rem 0.7rem;
    margin-bottom: 0.6rem;
  }
  .input {
    width: 100%;
    padding: 0.5rem 0.65rem;
    background: var(--bg-muted);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    color: var(--text-primary);
    font-size: 0.875rem;
  }
  .input:focus {
    outline: none;
    border-color: var(--sc-accent);
  }
  textarea.input {
    resize: vertical;
    font-family: var(--font-mono);
    line-height: 1.5;
  }
  .input.narrow {
    width: 9rem;
    flex: none;
  }
  .row-fields {
    display: flex;
    gap: 0.75rem;
  }
  .row-fields .field {
    flex: 1;
  }

  .trigger-picker {
    display: flex;
    gap: 0.4rem;
    flex-wrap: wrap;
  }
  .trigger-opt {
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
    padding: 0.45rem 0.7rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg-muted);
    color: var(--text-secondary);
    font-size: 0.8125rem;
    cursor: pointer;
  }
  .trigger-opt.active {
    border-color: var(--sc-accent);
    color: var(--text-primary);
    background: var(--sc-accent-tint);
  }

  .disclosure {
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
    background: none;
    border: none;
    color: var(--text-secondary);
    cursor: pointer;
    font-size: 0.8125rem;
    padding: 0;
  }
  .chevron {
    display: inline-flex;
    transition: transform var(--dur) var(--ease);
    color: var(--text-muted);
  }
  .chevron.open {
    transform: rotate(90deg);
  }
  .advanced {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
    padding: 0.75rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg-muted);
  }
  .editor-actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.5rem;
    margin-top: 0.25rem;
  }

  /* recipes */
  .recipe-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(190px, 1fr));
    gap: 0.6rem;
    margin-bottom: 1.25rem;
  }
  .tile {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
    padding: 0.85rem;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--bg-surface);
    text-align: left;
    cursor: pointer;
    transition: border-color var(--dur-fast) var(--ease);
  }
  .tile:hover {
    border-color: var(--sc-accent);
  }
  .tile-icon {
    display: inline-flex;
    color: var(--sc-accent);
  }
  .tile-name {
    font-size: 0.875rem;
    font-weight: 600;
  }
  .tile-blurb {
    font-size: 0.75rem;
    color: var(--text-muted);
  }

  /* list */
  .auto-list {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }
  .auto-card {
    padding: 0;
    overflow: hidden;
  }
  .auto-head {
    display: flex;
    justify-content: space-between;
    gap: 1rem;
    padding: 0.9rem 1rem;
  }
  .auto-main {
    display: flex;
    gap: 0.6rem;
    min-width: 0;
  }
  .trig-icon {
    display: inline-flex;
    color: var(--sc-accent);
    margin-top: 0.15rem;
    flex: none;
  }
  .auto-meta {
    min-width: 0;
  }
  .auto-name-row {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: wrap;
  }
  .auto-name-row h2 {
    font-size: 0.9375rem;
    font-weight: 600;
  }
  .auto-sub {
    font-size: 0.8125rem;
    color: var(--text-muted);
    margin-top: 0.15rem;
  }
  .auto-sub .folder {
    display: inline-flex;
    align-items: center;
    gap: 0.25rem;
    color: var(--text-secondary);
  }
  .auto-sub .folder :global(svg) {
    color: var(--sc-accent);
  }
  .auto-sub .folder.warn {
    color: var(--warning);
  }
  .auto-sub .folder.warn :global(svg) {
    color: var(--warning);
  }
  .auto-actions {
    display: flex;
    gap: 0.4rem;
    align-items: flex-start;
    flex: none;
  }
  .runs-toggle {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    width: 100%;
    padding: 0.55rem 1rem;
    background: none;
    border: none;
    border-top: 1px solid var(--border);
    color: var(--text-secondary);
    font-size: 0.8125rem;
    cursor: pointer;
    text-align: left;
  }
  .runs {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
    padding: 0.25rem 1rem 0.85rem;
  }
  .run-row {
    display: flex;
    align-items: center;
    gap: 0.75rem;
    padding: 0.45rem 0.6rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg-muted);
    cursor: pointer;
    text-align: left;
  }
  .run-row:hover {
    border-color: var(--border-strong);
  }
  .run-reason {
    font-size: 0.8125rem;
    color: var(--text-secondary);
    flex: 1;
  }
  .run-time {
    font-size: 0.75rem;
    color: var(--text-faint);
    font-family: var(--font-mono);
  }

  /* inspector */
  .overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    display: grid;
    place-items: center;
    z-index: 50;
    padding: 1.5rem;
  }
  .inspector {
    width: min(720px, 100%);
    max-height: 82vh;
    display: flex;
    flex-direction: column;
    background: var(--bg-surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow-lg);
    overflow: hidden;
  }
  .insp-head {
    display: flex;
    justify-content: space-between;
    align-items: center;
    gap: 1rem;
    padding: 0.9rem 1rem;
    border-bottom: 1px solid var(--border);
  }
  .insp-title {
    display: flex;
    align-items: center;
    gap: 0.6rem;
    min-width: 0;
  }
  .insp-title h2 {
    font-size: 0.9375rem;
    font-weight: 600;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .transcript {
    overflow-y: auto;
    padding: 1rem;
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
  }
  .t-thought {
    font-size: 0.8125rem;
    color: var(--text-muted);
    font-style: italic;
    border-left: 2px solid var(--border-strong);
    padding-left: 0.6rem;
    white-space: pre-wrap;
  }
  .t-text {
    font-size: 0.875rem;
    color: var(--text-primary);
    white-space: pre-wrap;
    line-height: 1.55;
  }
  .link {
    background: none;
    border: none;
    color: inherit;
    text-decoration: underline;
    cursor: pointer;
    font-size: inherit;
    padding: 0;
    margin-left: 0.4rem;
  }
  .raw-err {
    margin-top: 0.5rem;
    padding: 0.5rem;
    background: var(--bg-app);
    border-radius: var(--radius-sm);
    font-size: 0.75rem;
    white-space: pre-wrap;
    max-height: 8rem;
    overflow-y: auto;
  }
</style>

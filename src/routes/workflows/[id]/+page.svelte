<script lang="ts">
  import { onMount } from "svelte";
  import { page } from "$app/stores";
  import { goto } from "$app/navigation";
  import {
    SvelteFlow,
    Background,
    BackgroundVariant,
    Controls,
    MiniMap,
    type Node as FlowNode,
    type Edge as FlowEdge,
    type Connection,
  } from "@xyflow/svelte";
  import "@xyflow/svelte/dist/style.css";
  import Icon from "$lib/components/ui/Icon.svelte";
  import { subscribe } from "$lib/events";
  import WorkflowNode from "$lib/components/workflows/WorkflowNode.svelte";
  import NodePalette from "$lib/components/workflows/NodePalette.svelte";
  import Inspector from "$lib/components/workflows/Inspector.svelte";
  import PermissionsPanel from "$lib/components/workflows/PermissionsPanel.svelte";
  import RunsPanel from "$lib/components/workflows/RunsPanel.svelte";
  import {
    inTauri,
    wfApi,
    type CatalogEntry,
    type WfNode,
    type WfNodeFinished,
    type WfNodeStarted,
    type WfRunFinished,
    type WfRunRecord,
    type WfValidation,
    type WorkflowDoc,
  } from "$lib/workflows/api";
  import { makeNode, newWorkflowDoc } from "$lib/workflows/model";

  const nodeTypes = { wf: WorkflowNode } as never;

  let doc = $state<WorkflowDoc | null>(null);
  let catalog = $state<CatalogEntry[]>([]);
  let nodes = $state<FlowNode[]>([]);
  let edges = $state<FlowEdge[]>([]);
  let loading = $state(true);
  let dirty = $state(false);
  let saving = $state(false);
  let validation = $state<WfValidation | null>(null);
  let problemsOpen = $state(false);
  let permissionsOpen = $state(false);
  let selectedId = $state<string | null>(null);
  let rightTab = $state<"inspect" | "runs">("inspect");
  let runs = $state<WfRunRecord[]>([]);
  let selectedRun = $state<WfRunRecord | null>(null);
  let activeRunId = $state<string | null>(null);
  // The engine can emit the trigger + first node's events before runNow's IPC
  // reply sets activeRunId; this lets those early events adopt the run id.
  let awaitingRun = $state(false);
  let toast = $state<string | null>(null);
  let colorMode = $state<"dark" | "light">("dark");

  const specOf = (type: string) => catalog.find((c) => c.type === type);
  const selectedNode = $derived(
    selectedId ? (nodes.find((n) => n.id === selectedId)?.data as { def: WfNode; spec: CatalogEntry } | undefined) : undefined,
  );

  function flash(message: string) {
    toast = message;
    setTimeout(() => (toast = null), 2600);
  }

  function toFlow(w: WorkflowDoc) {
    nodes = w.nodes.map((def) => ({
      id: def.id,
      type: "wf",
      position: { x: def.position[0], y: def.position[1] },
      data: { def, spec: specOf(def.type), run: null },
    })) as FlowNode[];
    edges = w.connections.map((c, i) => ({
      id: `e${i}-${c.from}-${c.out}-${c.to}-${c.in}`,
      source: c.from,
      sourceHandle: c.out,
      target: c.to,
      targetHandle: c.in,
    })) as FlowEdge[];
  }

  /** Serialize the canvas back into the workflow document. */
  function toDoc(): WorkflowDoc {
    const w = doc!;
    w.nodes = nodes.map((n) => {
      const def = (n.data as { def: WfNode }).def;
      def.position = [Math.round(n.position.x), Math.round(n.position.y)];
      if (def.type === "trigger.schedule") {
        (def.params as Record<string, unknown>).tz_offset_minutes = new Date().getTimezoneOffset();
      }
      return def;
    });
    w.connections = edges.map((e) => ({
      from: e.source,
      out: e.sourceHandle ?? "main",
      to: e.target,
      in: e.targetHandle ?? "main",
    }));
    return w;
  }

  async function load() {
    const id = $page.params.id ?? "";
    try {
      catalog = await wfApi.catalog();
      const fetched = await wfApi.get(id);
      if (!fetched) {
        await goto("/workflows");
        return;
      }
      doc = fetched;
    } catch {
      // Plain-browser preview: no Tauri backend — show a sandbox graph.
      catalog = fallbackCatalog();
      doc = newWorkflowDoc("Sandbox preview");
      doc.id = "w-sandbox";
    }
    toFlow(doc!);
    await refreshRuns();
    loading = false;
  }

  async function refreshRuns() {
    if (!doc) return;
    try {
      runs = await wfApi.runs(doc.id);
    } catch {
      runs = [];
    }
  }

  async function save(): Promise<boolean> {
    if (!doc) return false;
    saving = true;
    // Snapshot and clear dirty now, before the awaits, so an edit made while the
    // save is in flight re-marks dirty instead of being masked as "Saved".
    const snapshot = $state.snapshot(toDoc()) as WorkflowDoc;
    dirty = false;
    try {
      const saved = await wfApi.save(snapshot);
      doc = { ...doc, ...saved, nodes: doc.nodes, connections: doc.connections };
      validation = await wfApi.validate(snapshot);
      return true;
    } catch (e) {
      dirty = true;
      flash(`Save failed: ${e}`);
      return false;
    } finally {
      saving = false;
    }
  }

  async function run() {
    if (!doc) return;
    if (!(await save())) return;
    if (validation && validation.errors.length > 0) {
      problemsOpen = true;
      flash("Fix the problems list before running");
      return;
    }
    try {
      clearRunOverlay();
      awaitingRun = true;
      edges = edges.map((e) => ({ ...e, animated: true, label: undefined }));
      rightTab = "runs";
      activeRunId = await wfApi.runNow(doc.id);
    } catch (e) {
      awaitingRun = false;
      flash(String(e));
    }
  }

  /** Accept a run event for this editor, adopting the run id from the first
   *  event when we're still awaiting runNow's reply. */
  function acceptRunEvent(workflowId: string, runId: string): boolean {
    if (!doc || workflowId !== doc.id) return false;
    if (activeRunId) return runId === activeRunId;
    if (awaitingRun) {
      activeRunId = runId;
      return true;
    }
    return false;
  }

  async function cancelActive() {
    if (activeRunId) {
      try {
        await wfApi.cancelRun(activeRunId);
      } catch (e) {
        flash(String(e));
      }
    }
  }

  function clearRunOverlay() {
    nodes = nodes.map((n) => ({ ...n, data: { ...(n.data as object), run: null } })) as FlowNode[];
    edges = edges.map((e) => ({ ...e, animated: false, label: undefined }));
    selectedRun = null;
  }

  function setNodeRun(nodeId: string, run: unknown) {
    nodes = nodes.map((n) => (n.id === nodeId ? ({ ...n, data: { ...(n.data as object), run } } as FlowNode) : n));
  }

  /** Paint a finished run's numbers onto the canvas (live or historical). */
  function applyRunOverlay(record: WfRunRecord) {
    for (const summary of record.nodes) {
      setNodeRun(summary.node_id, summary);
    }
    edges = edges.map((e) => {
      const captured = record.data?.[e.source]?.[e.sourceHandle ?? "main"];
      const count = captured?.total;
      return {
        ...e,
        animated: false,
        label: count != null && count > 0 ? `${count} item${count === 1 ? "" : "s"}` : undefined,
      };
    });
  }

  // ---------------------------------------------------------------- editing

  function addNode(entry: CatalogEntry) {
    if (!doc) return;
    const taken = new Set(nodes.map((n) => (n.data as { def: WfNode }).def.name));
    const rightmost = nodes.reduce((mx, n) => Math.max(mx, n.position.x), 40);
    const def = makeNode(entry, taken, [rightmost + 260, 220 + (nodes.length % 3) * 40]);
    nodes = [
      ...nodes,
      {
        id: def.id,
        type: "wf",
        position: { x: def.position[0], y: def.position[1] },
        data: { def, spec: entry, run: null },
        selected: false,
      } as FlowNode,
    ];
    selectedId = def.id;
    rightTab = "inspect";
    dirty = true;
  }

  function deleteSelected() {
    if (!selectedId) return;
    const id = selectedId;
    selectedId = null;
    nodes = nodes.filter((n) => n.id !== id);
    edges = edges.filter((e) => e.source !== id && e.target !== id);
    dirty = true;
  }

  function isValidConnection(conn: FlowEdge | Connection): boolean {
    if (conn.source === conn.target) return false;
    return !edges.some(
      (e) =>
        e.source === conn.source &&
        e.target === conn.target &&
        (e.sourceHandle ?? "main") === (conn.sourceHandle ?? "main") &&
        (e.targetHandle ?? "main") === (conn.targetHandle ?? "main"),
    );
  }

  function onconnect(conn: Connection) {
    dirty = true;
  }

  // ---------------------------------------------------------------- lifecycle

  onMount(() => {
    colorMode = document.documentElement.dataset.theme === "light" ? "light" : "dark";
    load();

    const offs = [
      subscribe<WfNodeStarted>("workflow-node-started", (e) => {
        if (!acceptRunEvent(e.payload.workflow_id, e.payload.run_id)) return;
        setNodeRun(e.payload.node_id, "running");
      }),
      subscribe<WfNodeFinished>("workflow-node-finished", (e) => {
        if (!acceptRunEvent(e.payload.workflow_id, e.payload.run_id)) return;
        setNodeRun(e.payload.node_id, {
          node_id: e.payload.node_id,
          name: e.payload.name,
          status: e.payload.status,
          items_in: e.payload.items_in,
          items_out: e.payload.items_out,
          duration_ms: e.payload.duration_ms,
          attempts: 1,
          error: e.payload.error ? { kind: "error", message: e.payload.error } : null,
        });
      }),
      subscribe<WfRunFinished>("workflow-run-finished", async (e) => {
        if (doc && e.payload.workflow_id === doc.id) {
          await refreshRuns();
          if (e.payload.run_id === activeRunId) {
            activeRunId = null;
            awaitingRun = false;
            const detail = await wfApi.runDetail(doc.id, e.payload.run_id).catch(() => null);
            if (detail) {
              applyRunOverlay(detail);
              selectedRun = detail;
            }
            if (e.payload.error) flash(e.payload.error);
          }
        }
      }),
    ];

    const keydown = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "s") {
        e.preventDefault();
        save();
      }
    };
    window.addEventListener("keydown", keydown);
    return () => {
      offs.forEach((off) => off());
      window.removeEventListener("keydown", keydown);
    };
  });

  async function selectRun(run: WfRunRecord) {
    if (!doc) return;
    const detail = await wfApi.runDetail(doc.id, run.id).catch(() => null);
    if (detail) {
      // clearRunOverlay() nulls selectedRun, so set it AFTER clearing.
      clearRunOverlay();
      selectedRun = detail;
      applyRunOverlay(detail);
    }
  }

  /** Minimal built-in catalog so the canvas renders in a plain browser. */
  function fallbackCatalog(): CatalogEntry[] {
    const needs = { network: false, code: false, fs_read: false, fs_write: false, agent: false };
    const port = (name: string, label: string) => ({ name, label });
    return [
      { type: "trigger.manual", type_version: 1, label: "Manual", category: "trigger", description: "", inputs: [], outputs: [port("main", "Out")], needs, is_trigger: true, secrets_ok: false },
      { type: "trigger.schedule", type_version: 1, label: "Schedule", category: "trigger", description: "", inputs: [], outputs: [port("main", "Out")], needs, is_trigger: true, secrets_ok: false },
      { type: "http.request", type_version: 1, label: "HTTP Request", category: "action", description: "", inputs: [port("main", "In")], outputs: [port("main", "Out")], needs: { ...needs, network: true }, is_trigger: false, secrets_ok: true },
      { type: "transform.set", type_version: 1, label: "Edit Fields", category: "transform", description: "", inputs: [port("main", "In")], outputs: [port("main", "Out")], needs, is_trigger: false, secrets_ok: false },
      { type: "flow.if", type_version: 1, label: "IF", category: "flow", description: "", inputs: [port("main", "In")], outputs: [port("true", "True"), port("false", "False")], needs, is_trigger: false, secrets_ok: false },
      { type: "flow.merge", type_version: 1, label: "Merge", category: "flow", description: "", inputs: [port("a", "A"), port("b", "B")], outputs: [port("main", "Out")], needs, is_trigger: false, secrets_ok: false },
      { type: "code.js", type_version: 1, label: "Code", category: "code", description: "", inputs: [port("main", "In")], outputs: [port("main", "Out")], needs: { ...needs, code: true }, is_trigger: false, secrets_ok: false },
      { type: "agent.run", type_version: 1, label: "Agent", category: "agent", description: "", inputs: [port("main", "In")], outputs: [port("main", "Out")], needs: { ...needs, agent: true }, is_trigger: false, secrets_ok: false },
      { type: "file.read", type_version: 1, label: "Read File", category: "action", description: "", inputs: [port("main", "In")], outputs: [port("main", "Out")], needs: { ...needs, fs_read: true }, is_trigger: false, secrets_ok: false },
      { type: "file.write", type_version: 1, label: "Write File", category: "action", description: "", inputs: [port("main", "In")], outputs: [port("main", "Out")], needs: { ...needs, fs_write: true }, is_trigger: false, secrets_ok: false },
      { type: "util.wait", type_version: 1, label: "Wait", category: "flow", description: "", inputs: [port("main", "In")], outputs: [port("main", "Out")], needs, is_trigger: false, secrets_ok: false },
    ];
  }
</script>

{#if loading}
  <div class="editor-loading">Loading…</div>
{:else if doc}
  <div class="editor">
    <header class="topbar">
      <button class="btn btn-ghost btn-icon" title="Back to workflows" onclick={() => goto("/workflows")}>
        <Icon name="chevron-right" size={14} />
      </button>
      <input class="wf-title" bind:value={doc.name} oninput={() => (dirty = true)} spellcheck="false" />
      <label class="enabled-toggle" title="Scheduled triggers only fire while enabled">
        <input type="checkbox" bind:checked={doc.enabled} onchange={() => (dirty = true)} />
        <span>{doc.enabled ? "Enabled" : "Disabled"}</span>
      </label>

      <div class="topbar-spacer"></div>

      {#if validation && (validation.errors.length > 0 || validation.warnings.length > 0)}
        <button
          class="btn btn-sm problems"
          class:has-errors={validation.errors.length > 0}
          onclick={() => (problemsOpen = !problemsOpen)}
        >
          {validation.errors.length > 0
            ? `${validation.errors.length} problem${validation.errors.length === 1 ? "" : "s"}`
            : `${validation.warnings.length} note${validation.warnings.length === 1 ? "" : "s"}`}
        </button>
      {/if}
      <button class="btn btn-sm" onclick={() => (permissionsOpen = true)}>
        <Icon name="shield" size={13} /> Permissions
      </button>
      <button class="btn btn-sm" disabled={saving || !dirty} onclick={save}>
        {saving ? "Saving…" : dirty ? "Save" : "Saved"}
      </button>
      {#if activeRunId}
        <button class="btn btn-sm stop-btn" onclick={cancelActive}><Icon name="stop" size={13} /> Stop</button>
      {:else}
        <button class="btn btn-accent btn-sm" onclick={run}><Icon name="play" size={13} /> Run</button>
      {/if}
    </header>

    <div class="body">
      <NodePalette {catalog} onadd={addNode} />

      <div class="canvas">
        <SvelteFlow
          bind:nodes
          bind:edges
          {nodeTypes}
          {colorMode}
          {isValidConnection}
          {onconnect}
          fitView
          fitViewOptions={{ padding: 0.25, maxZoom: 1.1 }}
          minZoom={0.25}
          maxZoom={1.75}
          deleteKey={["Delete"]}
          defaultEdgeOptions={{ type: "default" }}
          onnodeclick={({ node }) => {
            selectedId = node.id;
            rightTab = "inspect";
          }}
          onpaneclick={() => (selectedId = null)}
          onnodedragstop={() => (dirty = true)}
          ondelete={() => {
            selectedId = null;
            dirty = true;
          }}
        >
          <Background variant={BackgroundVariant.Dots} gap={22} size={1.2} />
          <Controls showLock={false} />
          <MiniMap pannable zoomable width={160} height={110} />
        </SvelteFlow>

        {#if problemsOpen && validation}
          <div class="problems-panel">
            <div class="problems-head">
              <span class="mono-label">Problems</span>
              <button class="btn btn-ghost btn-icon btn-sm" onclick={() => (problemsOpen = false)}>
                <Icon name="close" size={12} />
              </button>
            </div>
            {#each validation.errors as item, i (i)}
              <button class="problem error" onclick={() => item.node_id && (selectedId = item.node_id)}>
                {item.message}
              </button>
            {/each}
            {#each validation.warnings as item, i (i)}
              <button class="problem warning" onclick={() => item.node_id && (selectedId = item.node_id)}>
                {item.message}
              </button>
            {/each}
            {#if validation.errors.length === 0 && validation.warnings.length === 0}
              <div class="problem ok">No problems. Ready to run.</div>
            {/if}
          </div>
        {/if}

        {#if toast}
          <div class="toast">{toast}</div>
        {/if}
      </div>

      <aside class="right">
        <div class="right-tabs">
          <button class="tab" class:active={rightTab === "inspect"} onclick={() => (rightTab = "inspect")}>Node</button>
          <button class="tab" class:active={rightTab === "runs"} onclick={() => (rightTab = "runs")}>
            Runs{runs.length ? ` (${runs.length})` : ""}
          </button>
        </div>
        {#if rightTab === "inspect"}
          {#if selectedNode && selectedNode.spec}
            <Inspector
              node={selectedNode.def}
              spec={selectedNode.spec}
              onchange={() => (dirty = true)}
              ondelete={deleteSelected}
            />
          {:else}
            <div class="right-empty">
              <div class="mono-label">Workflow settings</div>
              <label class="setting">
                <span>Run time limit (seconds)</span>
                <input type="number" bind:value={doc.settings.timeout_secs} oninput={() => (dirty = true)} />
              </label>
              <label class="setting">
                <span>If a run is already going</span>
                <select bind:value={doc.settings.overlap} onchange={() => (dirty = true)}>
                  <option value="skip">Skip the new one</option>
                  <option value="replace">Stop it and start fresh</option>
                </select>
              </label>
              <label class="setting">
                <span>Keep run data</span>
                <select bind:value={doc.settings.capture} onchange={() => (dirty = true)}>
                  <option value="sample">Sample (first 20 items)</option>
                  <option value="full">Everything</option>
                  <option value="none">Nothing</option>
                </select>
              </label>
              <p class="hint">Click a node to edit it. Drag between the port dots to connect nodes.</p>
            </div>
          {/if}
        {:else}
          <RunsPanel
            {runs}
            selected={selectedRun}
            onselect={selectRun}
            oncancel={(id) => wfApi.cancelRun(id).catch((e) => flash(String(e)))}
            onrefresh={refreshRuns}
          />
        {/if}
      </aside>
    </div>
  </div>

  {#if permissionsOpen}
    <PermissionsPanel
      permissions={doc.permissions}
      nodes={nodes.map((n) => (n.data as { def: WfNode }).def)}
      {catalog}
      onchange={() => (dirty = true)}
      onclose={() => (permissionsOpen = false)}
    />
  {/if}
{/if}

<style>
  .editor-loading {
    display: grid;
    place-items: center;
    height: 100%;
    color: var(--text-muted);
  }

  .editor {
    display: flex;
    flex-direction: column;
    height: 100%;
    min-height: 0;
  }

  .topbar {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.55rem 0.9rem;
    border-bottom: 1px solid var(--border);
    background: var(--bg-sidebar);
  }

  .topbar :global(.btn-icon svg) {
    transform: rotate(180deg);
  }

  .wf-title {
    width: min(340px, 30vw);
    padding: 0.3rem 0.5rem;
    font-size: 0.9375rem;
    font-weight: 650;
    background: transparent;
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
  }

  .wf-title:hover,
  .wf-title:focus {
    border-color: var(--border);
    background: var(--bg-surface-2);
    outline: none;
  }

  .enabled-toggle {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    font-size: 0.75rem;
    color: var(--text-muted);
    cursor: pointer;
    white-space: nowrap;
  }

  .topbar-spacer {
    flex: 1;
  }

  .problems {
    color: var(--warning);
    border-color: color-mix(in srgb, var(--warning) 35%, var(--border));
  }

  .problems.has-errors {
    color: var(--danger);
    border-color: color-mix(in srgb, var(--danger) 40%, var(--border));
  }

  .stop-btn {
    color: var(--danger);
    border-color: color-mix(in srgb, var(--danger) 40%, var(--border));
  }

  .body {
    display: flex;
    flex: 1;
    min-height: 0;
  }

  .canvas {
    position: relative;
    flex: 1;
    min-width: 0;
  }

  .right {
    display: flex;
    flex-direction: column;
    width: 330px;
    flex: none;
    min-height: 0;
    border-left: 1px solid var(--border);
    background: var(--bg-sidebar);
  }

  .right-tabs {
    display: flex;
    gap: 0.25rem;
    padding: 0.55rem 0.6rem 0;
  }

  .tab {
    padding: 0.35rem 0.7rem;
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--text-muted);
    font-size: 0.78125rem;
    font-weight: 600;
    cursor: pointer;
  }

  .tab:hover {
    background: var(--bg-hover);
  }

  .tab.active {
    background: var(--bg-surface-2);
    border-color: var(--border);
    color: var(--text-primary);
  }

  .right > :global(.inspector),
  .right > :global(.runs) {
    flex: 1;
    min-height: 0;
  }

  .right-empty {
    display: flex;
    flex-direction: column;
    gap: 0.7rem;
    padding: 0.9rem;
  }

  .setting {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
    font-size: 0.75rem;
    font-weight: 600;
    color: var(--text-secondary);
  }

  .setting input,
  .setting select {
    padding: 0.42rem 0.55rem;
    background: var(--bg-surface-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    font-size: 0.8125rem;
    font-weight: 400;
  }

  .hint {
    font-size: 0.75rem;
    color: var(--text-faint);
    line-height: 1.5;
  }

  .problems-panel {
    position: absolute;
    left: 0.9rem;
    bottom: 0.9rem;
    z-index: 10;
    width: min(380px, 60%);
    max-height: 40%;
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    padding: 0.6rem;
    background: var(--bg-surface);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    box-shadow: var(--shadow-md);
  }

  .problems-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .problem {
    padding: 0.35rem 0.5rem;
    border: none;
    border-left: 2px solid var(--border);
    border-radius: 4px;
    background: transparent;
    text-align: left;
    font-size: 0.75rem;
    color: var(--text-secondary);
    cursor: pointer;
    line-height: 1.4;
  }

  .problem:hover {
    background: var(--bg-hover);
  }

  .problem.error {
    border-left-color: var(--danger);
  }

  .problem.warning {
    border-left-color: var(--warning);
  }

  .problem.ok {
    border-left-color: var(--success);
    cursor: default;
  }

  .toast {
    position: absolute;
    left: 50%;
    bottom: 1.1rem;
    transform: translateX(-50%);
    z-index: 20;
    padding: 0.5rem 0.9rem;
    background: var(--bg-surface-2);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    box-shadow: var(--shadow-md);
    font-size: 0.8125rem;
    color: var(--text-primary);
    max-width: 70%;
  }

  /* ------------------------------------------------ Svelte Flow theming --- */

  .canvas :global(.svelte-flow) {
    background: var(--bg-app) !important;
  }

  .canvas :global(.svelte-flow__background pattern circle) {
    fill: var(--border-strong);
  }

  .canvas :global(.svelte-flow__edge-path) {
    stroke: var(--border-strong);
    stroke-width: 1.6;
  }

  .canvas :global(.svelte-flow__edge.selected .svelte-flow__edge-path),
  .canvas :global(.svelte-flow__edge:hover .svelte-flow__edge-path) {
    stroke: var(--accent);
  }

  .canvas :global(.svelte-flow__edge.animated .svelte-flow__edge-path) {
    stroke: var(--accent);
    stroke-dasharray: 6;
    animation: wf-dash 0.6s linear infinite;
  }

  @keyframes wf-dash {
    to {
      stroke-dashoffset: -12;
    }
  }

  .canvas :global(.svelte-flow__edgelabel) {
    background: var(--bg-surface-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-pill);
    padding: 0.05rem 0.45rem;
    font-size: 0.625rem;
    font-weight: 600;
    color: var(--text-secondary);
  }

  .canvas :global(.svelte-flow__connectionline path) {
    stroke: var(--accent);
    stroke-width: 1.6;
    stroke-dasharray: 5;
  }

  .canvas :global(.svelte-flow__controls) {
    box-shadow: var(--shadow-sm);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    overflow: hidden;
  }

  .canvas :global(.svelte-flow__controls-button) {
    background: var(--bg-surface);
    border-bottom: 1px solid var(--border);
    color: var(--text-secondary);
    fill: var(--text-secondary);
  }

  .canvas :global(.svelte-flow__controls-button:hover) {
    background: var(--bg-surface-2);
  }

  .canvas :global(.svelte-flow__minimap) {
    background: var(--bg-surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    overflow: hidden;
  }

  .canvas :global(.svelte-flow__minimap-mask) {
    fill: color-mix(in srgb, var(--bg-app) 70%, transparent);
  }

  .canvas :global(.svelte-flow__minimap-node) {
    fill: var(--border-strong);
  }

  .canvas :global(.svelte-flow__attribution) {
    background: transparent;
    color: var(--text-faint);
  }
</style>

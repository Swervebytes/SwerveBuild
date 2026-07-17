<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { open } from "@tauri-apps/plugin-dialog";
  import Icon from "$lib/components/ui/Icon.svelte";
  import StatusPill from "$lib/components/ui/StatusPill.svelte";

  type ServerStatus = {
    state: "stopped" | "starting" | "ready" | "failed";
    model_id: string | null;
    port: number | null;
    message: string | null;
  };
  type LocalModel = {
    id: string;
    label: string;
    path: string;
    size_bytes: number;
    added_at: string;
  };
  type LocalState = {
    engine_installed: boolean;
    engine_version: string;
    server: ServerStatus;
    models: LocalModel[];
  };
  type EngineProgress = { phase: string; received: number; total: number };

  let local = $state<LocalState | null>(null);
  let installing = $state(false);
  let progress = $state<EngineProgress | null>(null);
  let busyModel = $state<string | null>(null);
  let message = $state<string | null>(null);
  let isError = $state(false);

  let unlisteners: Array<() => void> = [];

  async function refresh() {
    try {
      local = await invoke<LocalState>("get_local_state");
    } catch (e) {
      message = String(e);
      isError = true;
    }
  }

  onMount(() => {
    refresh();
    listen<EngineProgress>("local-engine-progress", (ev) => {
      progress = ev.payload;
    }).then((u) => unlisteners.push(u));
    listen<ServerStatus>("local-llm-status", (ev) => {
      if (local) local = { ...local, server: ev.payload };
    }).then((u) => unlisteners.push(u));
    return () => unlisteners.forEach((u) => u());
  });

  async function installEngine() {
    installing = true;
    message = null;
    isError = false;
    progress = { phase: "downloading", received: 0, total: 1 };
    try {
      const r = await invoke<{ success: boolean; message: string }>("install_local_engine");
      message = r.message;
      await refresh();
    } catch (e) {
      message = String(e);
      isError = true;
    } finally {
      installing = false;
      progress = null;
    }
  }

  async function addModel() {
    message = null;
    isError = false;
    const selected = await open({
      multiple: false,
      title: "Pick a GGUF model file",
      filters: [{ name: "GGUF model", extensions: ["gguf"] }],
    });
    if (!selected || typeof selected !== "string") return;
    try {
      local = await invoke<LocalState>("add_local_model", { path: selected });
      message = "Model added — it now appears in every model picker.";
    } catch (e) {
      message = String(e);
      isError = true;
    }
  }

  async function removeModel(id: string) {
    message = null;
    isError = false;
    try {
      local = await invoke<LocalState>("remove_local_model", { id });
    } catch (e) {
      message = String(e);
      isError = true;
    }
  }

  async function preload(id: string) {
    busyModel = id;
    message = null;
    isError = false;
    try {
      local = await invoke<LocalState>("start_local_server", { modelId: id });
    } catch (e) {
      message = String(e);
      isError = true;
      await refresh();
    } finally {
      busyModel = null;
    }
  }

  async function stopServer() {
    try {
      local = await invoke<LocalState>("stop_local_server");
    } catch (e) {
      message = String(e);
      isError = true;
    }
  }

  function humanSize(bytes: number): string {
    if (bytes >= 1e9) return `${(bytes / 1e9).toFixed(1)} GB`;
    if (bytes >= 1e6) return `${Math.round(bytes / 1e6)} MB`;
    return `${bytes} B`;
  }

  const pct = $derived(
    progress && progress.total > 0
      ? Math.min(100, Math.round((progress.received / progress.total) * 100))
      : 0,
  );

  function serverTone(s: ServerStatus["state"]): "success" | "warning" | "danger" | "muted" {
    if (s === "ready") return "success";
    if (s === "starting") return "warning";
    if (s === "failed") return "danger";
    return "muted";
  }
  function serverLabel(server: ServerStatus): string {
    if (server.state === "ready") return `Serving ${server.model_id ?? "?"}`;
    if (server.state === "starting") return `Loading ${server.model_id ?? "…"}`;
    if (server.state === "failed") return "Failed";
    return "Stopped";
  }
</script>

{#if local}
  <div class="row engine-row">
    {#if local.engine_installed}
      <StatusPill tone="success" label={`Engine ${local.engine_version}`} />
      <StatusPill tone={serverTone(local.server.state)} label={serverLabel(local.server)} pulse={local.server.state === "starting"} />
      {#if local.server.state === "ready" || local.server.state === "starting"}
        <button class="btn btn-sm btn-ghost" type="button" onclick={stopServer}>Stop</button>
      {/if}
    {:else}
      <StatusPill tone="muted" label="Engine not installed" />
      <button class="btn btn-sm" type="button" disabled={installing} onclick={installEngine}>
        {installing ? "Installing…" : "Install engine (~33 MB)"}
      </button>
    {/if}
  </div>

  {#if installing && progress}
    <div class="progress">
      <div class="bar" style="width: {pct}%"></div>
    </div>
    <p class="progress-note mono-label">
      {progress.phase} · {pct}%
    </p>
  {/if}

  {#if local.models.length > 0}
    <div class="models">
      {#each local.models as m (m.id)}
        <div class="model-row">
          <div class="meta">
            <span class="name">{m.label}</span>
            <span class="sub mono-label">{m.id} · {humanSize(m.size_bytes)}</span>
          </div>
          <div class="btns">
            {#if local.server.model_id === m.id && local.server.state === "ready"}
              <StatusPill tone="success" label="Loaded" />
            {:else}
              <button
                class="btn btn-sm btn-ghost"
                type="button"
                disabled={busyModel !== null || !local.engine_installed}
                onclick={() => preload(m.id)}
              >
                {busyModel === m.id ? "Loading…" : "Load"}
              </button>
            {/if}
            <button
              class="btn btn-sm btn-ghost"
              type="button"
              title="Remove from Swerve (the file stays on disk)"
              onclick={() => removeModel(m.id)}
            >
              <Icon name="trash" size={13} />
            </button>
          </div>
        </div>
      {/each}
    </div>
  {:else}
    <p class="empty-note">
      No local models yet. Add any <code>.gguf</code> file — it becomes a model you can pick in any
      chat or automation, served entirely on this machine.
    </p>
  {/if}

  <div class="actions">
    <button class="btn btn-sm" type="button" onclick={addModel}>
      <Icon name="plus" size={13} />
      Add GGUF model…
    </button>
  </div>

  {#if local.server.state === "failed" && local.server.message}
    <p class="msg error">{local.server.message}</p>
  {/if}
  {#if message}
    <p class="msg" class:error={isError}>{message}</p>
  {/if}
{:else}
  <p class="empty-note">Loading local model state…</p>
{/if}

<style>
  .row {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: wrap;
  }
  .engine-row {
    margin-bottom: 0.75rem;
  }

  .progress {
    height: 6px;
    border-radius: var(--radius-pill);
    background: var(--bg-muted);
    overflow: hidden;
    margin-top: 0.25rem;
  }
  .bar {
    height: 100%;
    background: var(--sc-accent);
    border-radius: var(--radius-pill);
    transition: width 300ms var(--ease);
  }
  .progress-note {
    margin-top: 0.35rem;
    font-size: 0.6875rem;
    color: var(--text-muted);
  }

  .models {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
    margin-top: 0.5rem;
  }
  .model-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem;
    padding: 0.6rem 0.75rem;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--bg-muted);
  }
  .meta {
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 0.15rem;
  }
  .name {
    font-weight: 600;
    font-size: 0.8375rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .sub {
    font-size: 0.6875rem;
    color: var(--text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .btns {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    flex: none;
  }

  .empty-note {
    font-size: 0.8125rem;
    color: var(--text-muted);
    line-height: 1.5;
    margin-top: 0.25rem;
  }
  .empty-note code {
    font-family: var(--font-mono);
    font-size: 0.85em;
  }
  .actions {
    margin-top: 0.75rem;
  }
  .msg {
    font-size: 0.8125rem;
    color: var(--text-secondary);
    margin-top: 0.6rem;
    word-break: break-word;
  }
  .msg.error {
    color: var(--danger, #ff6b6b);
  }
</style>

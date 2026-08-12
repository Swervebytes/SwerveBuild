<script lang="ts">
  import { onMount } from "svelte";
  import { goto } from "$app/navigation";
  import Icon from "$lib/components/ui/Icon.svelte";
  import EmptyState from "$lib/components/ui/EmptyState.svelte";
  import StatusPill from "$lib/components/ui/StatusPill.svelte";
  import { subscribe } from "$lib/events";
  import { inTauri, wfApi, type WfRunFinished, type WorkflowDoc } from "$lib/workflows/api";
  import {
    fromEpoch,
    newWorkflowDoc,
    runStatusLabel,
    runStatusTone,
    workflowTriggerSummary,
  } from "$lib/workflows/model";

  let loading = $state(true);
  let workflows = $state<WorkflowDoc[]>([]);
  let paused = $state(false);
  let busy = $state<string | null>(null);
  let confirmDelete = $state<WorkflowDoc | null>(null);

  // Secrets manager
  let secretsOpen = $state(false);
  let secretNames = $state<string[]>([]);
  let newSecretName = $state("");
  let newSecretValue = $state("");
  let secretError = $state<string | null>(null);

  async function refresh() {
    try {
      workflows = await wfApi.list();
      paused = await wfApi.getPaused();
    } catch {
      workflows = [];
    }
  }

  onMount(() => {
    refresh().then(() => (loading = false));
    return subscribe<WfRunFinished>("workflow-run-finished", () => {
      refresh();
    });
  });

  async function createWorkflow() {
    busy = "new";
    try {
      const saved = await wfApi.save(newWorkflowDoc("New workflow"));
      await goto(`/workflows/${saved.id}`);
    } catch (e) {
      console.error(e);
    } finally {
      busy = null;
    }
  }

  async function runNow(w: WorkflowDoc) {
    busy = w.id;
    try {
      await wfApi.runNow(w.id);
      await refresh();
    } catch (e) {
      console.error(e);
    } finally {
      busy = null;
    }
  }

  async function toggleEnabled(w: WorkflowDoc) {
    w.enabled = !w.enabled;
    try {
      await wfApi.save($state.snapshot(w) as WorkflowDoc);
    } catch {
      w.enabled = !w.enabled;
    }
  }

  async function togglePaused() {
    paused = !paused;
    try {
      await wfApi.setPaused(paused);
    } catch {
      paused = !paused;
    }
  }

  async function doDelete() {
    if (!confirmDelete) return;
    try {
      await wfApi.remove(confirmDelete.id);
      confirmDelete = null;
      await refresh();
    } catch (e) {
      console.error(e);
    }
  }

  async function openSecrets() {
    secretsOpen = true;
    secretError = null;
    try {
      secretNames = await wfApi.secretNames();
    } catch {
      secretNames = [];
    }
  }

  async function addSecret() {
    secretError = null;
    try {
      await wfApi.secretSet(newSecretName, newSecretValue);
      newSecretName = "";
      newSecretValue = "";
      secretNames = await wfApi.secretNames();
    } catch (e) {
      secretError = String(e);
    }
  }

  async function dropSecret(name: string) {
    try {
      await wfApi.secretDelete(name);
      secretNames = await wfApi.secretNames();
    } catch (e) {
      secretError = String(e);
    }
  }

  const browserPreview = !inTauri();
</script>

<div class="page">
  <header class="head">
    <div>
      <h1 class="page-title">Workflows</h1>
      <p class="page-subtitle">Visual pipelines: triggers, requests, transforms, code, and agent turns.</p>
    </div>
    <div class="head-actions">
      <button class="btn btn-sm" class:paused onclick={togglePaused}
        title={paused ? "Resume all scheduled workflows" : "Pause all scheduled workflows"}>
        <Icon name={paused ? "play" : "pause"} size={13} />
        {paused ? "Resume all" : "Pause all"}
      </button>
      <button class="btn btn-sm" onclick={openSecrets}><Icon name="key" size={13} /> Secrets</button>
      <button class="btn btn-primary" disabled={busy === "new"} onclick={createWorkflow}>
        <Icon name="plus" size={14} /> New workflow
      </button>
    </div>
  </header>

  {#if browserPreview}
    <div class="preview-note">Browser preview: the engine lives in the desktop app, so data here is empty.</div>
  {/if}

  {#if loading}
    <p class="loading">Loading…</p>
  {:else if workflows.length === 0}
    <EmptyState icon="workflow" title="No workflows yet"
      description="Build your first pipeline: a trigger, a few nodes, wired together on a canvas.">
      <button class="btn btn-primary" onclick={createWorkflow}><Icon name="plus" size={14} /> New workflow</button>
    </EmptyState>
  {:else}
    <div class="grid">
      {#each workflows as w (w.id)}
        <div class="wf-card" class:off={!w.enabled}>
          <button class="wf-card-main" onclick={() => goto(`/workflows/${w.id}`)}>
            <div class="wf-card-top">
              <span class="wf-icon"><Icon name="workflow" size={16} /></span>
              <span class="wf-name">{w.name}</span>
              <StatusPill tone={runStatusTone(w.state.last_status)} label={runStatusLabel(w.state.last_status)}
                pulse={w.state.last_status === "running"} />
            </div>
            <div class="wf-meta">
              <span>{workflowTriggerSummary(w)}</span>
              <span class="dot-sep">·</span>
              <span>{w.nodes.length} node{w.nodes.length === 1 ? "" : "s"}</span>
              {#if w.updated_at}
                <span class="dot-sep">·</span>
                <span>updated {fromEpoch(w.updated_at)}</span>
              {/if}
            </div>
          </button>
          <div class="wf-card-actions">
            <label class="enable" title={w.enabled ? "Enabled" : "Disabled"}>
              <input type="checkbox" checked={w.enabled} onchange={() => toggleEnabled(w)} />
              <span>{w.enabled ? "On" : "Off"}</span>
            </label>
            <div class="spacer"></div>
            <button class="btn btn-ghost btn-sm" disabled={busy === w.id} onclick={() => runNow(w)}>
              <Icon name="play" size={12} /> Run
            </button>
            <button class="btn btn-ghost btn-icon btn-sm" title="Delete" onclick={() => (confirmDelete = w)}>
              <Icon name="trash" size={13} />
            </button>
          </div>
        </div>
      {/each}
    </div>
  {/if}
</div>

<svelte:window onkeydown={(e) => {
  if (e.key === "Escape") {
    confirmDelete = null;
    secretsOpen = false;
  }
}} />

{#if confirmDelete}
  <div class="scrim" role="presentation" onclick={(e) => e.target === e.currentTarget && (confirmDelete = null)}>
    <div class="dialog" role="dialog" aria-modal="true" tabindex="-1">
      <h2 class="dialog-title">Delete {confirmDelete.name}?</h2>
      <p class="dialog-text">The workflow and its run history are removed. There is no undo.</p>
      <div class="dialog-actions">
        <button class="btn" onclick={() => (confirmDelete = null)}>Keep it</button>
        <button class="btn danger" onclick={doDelete}><Icon name="trash" size={13} /> Delete</button>
      </div>
    </div>
  </div>
{/if}

{#if secretsOpen}
  <div class="scrim" role="presentation" onclick={(e) => e.target === e.currentTarget && (secretsOpen = false)}>
    <div class="dialog" role="dialog" aria-modal="true" tabindex="-1">
      <h2 class="dialog-title"><Icon name="key" size={14} /> Workflow secrets</h2>
      <p class="dialog-text">
        Named values for HTTP nodes via <code>{'{{ $secret("name") }}'}</code>. Stored locally in your profile.
      </p>
      {#if secretNames.length > 0}
        <div class="secret-list">
          {#each secretNames as name (name)}
            <div class="secret-row">
              <span class="secret-name">{name}</span>
              <span class="secret-mask">••••••••</span>
              <button class="btn btn-ghost btn-icon btn-sm" title="Delete secret" onclick={() => dropSecret(name)}>
                <Icon name="trash" size={12} />
              </button>
            </div>
          {/each}
        </div>
      {/if}
      <div class="secret-add">
        <input placeholder="name" bind:value={newSecretName} spellcheck="false" />
        <input placeholder="value" type="password" bind:value={newSecretValue} />
        <button class="btn btn-sm" disabled={!newSecretName.trim()} onclick={addSecret}>
          <Icon name="plus" size={12} /> Add
        </button>
      </div>
      {#if secretError}<p class="secret-error">{secretError}</p>{/if}
    </div>
  </div>
{/if}

<style>
  .page {
    height: 100%;
    overflow-y: auto;
    padding: 1.5rem 1.75rem 2.5rem;
  }

  .head {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 1rem;
    margin-bottom: 1.25rem;
  }

  .head-actions {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }

  .paused {
    color: var(--warning);
    border-color: color-mix(in srgb, var(--warning) 40%, var(--border));
  }

  .preview-note {
    margin-bottom: 1rem;
    padding: 0.5rem 0.75rem;
    border: 1px dashed var(--border-strong);
    border-radius: var(--radius-sm);
    font-size: 0.75rem;
    color: var(--text-muted);
  }

  .loading {
    color: var(--text-muted);
  }

  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(310px, 1fr));
    gap: 0.9rem;
  }

  .wf-card {
    display: flex;
    flex-direction: column;
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    background: var(--bg-surface);
    box-shadow: var(--shadow-sm);
    overflow: hidden;
    transition: border-color var(--dur-fast) var(--ease), box-shadow var(--dur-fast) var(--ease);
  }

  .wf-card:hover {
    border-color: var(--border-strong);
    box-shadow: var(--shadow-md);
  }

  .wf-card.off {
    opacity: 0.65;
  }

  .wf-card-main {
    display: block;
    width: 100%;
    padding: 0.9rem 1rem 0.6rem;
    background: transparent;
    border: none;
    cursor: pointer;
    text-align: left;
  }

  .wf-card-top {
    display: flex;
    align-items: center;
    gap: 0.55rem;
  }

  .wf-icon {
    display: grid;
    place-items: center;
    width: 30px;
    height: 30px;
    flex: none;
    border-radius: var(--radius-sm);
    background: var(--accent-tint);
    color: var(--accent);
  }

  .wf-name {
    flex: 1;
    min-width: 0;
    font-size: 0.9375rem;
    font-weight: 650;
    color: var(--text-primary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .wf-meta {
    margin-top: 0.5rem;
    font-size: 0.75rem;
    color: var(--text-muted);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .dot-sep {
    margin: 0 0.3rem;
    color: var(--text-faint);
  }

  .wf-card-actions {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    padding: 0.45rem 0.75rem;
    border-top: 1px solid var(--border);
    background: var(--bg-muted);
  }

  .enable {
    display: flex;
    align-items: center;
    gap: 0.35rem;
    font-size: 0.71875rem;
    color: var(--text-muted);
    cursor: pointer;
  }

  .spacer {
    flex: 1;
  }

  .scrim {
    position: fixed;
    inset: 0;
    z-index: 60;
    display: grid;
    place-items: center;
    background: rgba(0, 0, 0, 0.55);
    backdrop-filter: blur(2px);
  }

  .dialog {
    width: min(440px, calc(100vw - 3rem));
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
    padding: 1.1rem;
    background: var(--bg-surface);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow-lg);
  }

  .dialog-title {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    font-size: 0.9375rem;
    font-weight: 700;
  }

  .dialog-text {
    font-size: 0.8125rem;
    color: var(--text-muted);
    line-height: 1.45;
  }

  .dialog-text code {
    font-family: var(--font-mono);
    font-size: 0.71875rem;
    background: var(--bg-muted);
    padding: 0.1rem 0.3rem;
    border-radius: 5px;
  }

  .dialog-actions {
    display: flex;
    justify-content: flex-end;
    gap: 0.5rem;
    margin-top: 0.3rem;
  }

  .danger {
    background: var(--danger-tint);
    border-color: color-mix(in srgb, var(--danger) 45%, transparent);
    color: var(--danger);
  }

  .secret-list {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
  }

  .secret-row {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.35rem 0.55rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg-muted);
  }

  .secret-name {
    flex: 1;
    font-family: var(--font-mono);
    font-size: 0.75rem;
    color: var(--text-primary);
  }

  .secret-mask {
    color: var(--text-faint);
    font-size: 0.75rem;
  }

  .secret-add {
    display: grid;
    grid-template-columns: 1fr 1.3fr auto;
    gap: 0.4rem;
  }

  .secret-add input {
    padding: 0.42rem 0.55rem;
    background: var(--bg-surface-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    font-size: 0.8125rem;
  }

  .secret-error {
    font-size: 0.75rem;
    color: var(--danger);
  }
</style>

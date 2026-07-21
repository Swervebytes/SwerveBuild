<script lang="ts">
  import StatusPill from "$lib/components/ui/StatusPill.svelte";
  import ProviderPicker from "$lib/components/providers/ProviderPicker.svelte";
  import ModelPicker from "$lib/components/models/ModelPicker.svelte";
  import { browserPane } from "$lib/stores/browserPane.svelte";

  let {
    title,
    projectName,
    projectPath,
    connected,
    activeSessionCount = 0,
    showModelPicker = false,
    modelId = null,
    modelSwitching = false,
    onmodelchange,
  }: {
    title: string;
    projectName?: string;
    projectPath?: string;
    connected: boolean;
    activeSessionCount?: number;
    /** Model picker only makes sense for the Grok provider (`-m` is grok's flag). */
    showModelPicker?: boolean;
    modelId?: string | null;
    modelSwitching?: boolean;
    onmodelchange?: (id: string | null) => void;
  } = $props();
</script>

<header class="chat-header">
  <div class="titles">
    <h1 class="title">{title}</h1>
    <p class="subtitle">
      {#if projectName}
        <span class="proj">{projectName}</span>
        {#if projectPath}<span class="path">{projectPath}</span>{/if}
      {:else}
        Loading project…
      {/if}
    </p>
  </div>

  <div class="actions">
    {#if connected}
      <StatusPill tone="success" label="Connected" />
    {:else}
      <StatusPill tone="warning" label="Connecting…" pulse />
    {/if}
    {#if activeSessionCount > 0}
      <span class="badge badge-muted">{activeSessionCount}/3</span>
    {/if}
    {#if showModelPicker && onmodelchange}
      <ModelPicker value={modelId} disabled={modelSwitching} onchange={onmodelchange} />
    {/if}
    <ProviderPicker />
    <button
      class="browser-toggle"
      class:active={browserPane.open}
      type="button"
      onclick={() => browserPane.toggle()}
      aria-label="Toggle preview browser"
      aria-pressed={browserPane.open}
      title="Preview browser — open a localhost or web page here"
    >
      <svg viewBox="0 0 16 16" width="15" height="15" aria-hidden="true">
        <circle cx="8" cy="8" r="6.25" />
        <path d="M1.75 8h12.5M8 1.75c1.9 2 1.9 10.5 0 12.5M8 1.75c-1.9 2-1.9 10.5 0 12.5" />
      </svg>
    </button>
    <a class="btn btn-ghost btn-sm" href="/projects">All chats</a>
  </div>
</header>

<style>
  .chat-header {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 1rem;
    padding-bottom: 0.9rem;
    margin-bottom: 0.25rem;
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
  }
  .titles {
    min-width: 0;
  }
  .title {
    font-size: 1.125rem;
    font-weight: 700;
    letter-spacing: -0.01em;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .subtitle {
    margin-top: 0.2rem;
    display: flex;
    gap: 0.5rem;
    align-items: baseline;
    font-size: 0.8125rem;
    color: var(--text-secondary);
    min-width: 0;
  }
  .path {
    font-family: var(--font-mono);
    font-size: 0.75rem;
    color: var(--text-faint);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-shrink: 0;
  }
  .browser-toggle {
    display: grid;
    place-items: center;
    width: 30px;
    height: 30px;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm, 6px);
    background: var(--bg-surface);
    color: var(--text-secondary);
    cursor: pointer;
    transition:
      background var(--dur-fast) var(--ease),
      color var(--dur-fast) var(--ease),
      border-color var(--dur-fast) var(--ease);
  }
  .browser-toggle svg {
    stroke: currentColor;
    stroke-width: 1.3;
    fill: none;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  .browser-toggle:hover {
    color: var(--text-primary);
    border-color: var(--sc-accent);
  }
  .browser-toggle.active {
    color: var(--sc-accent);
    border-color: var(--sc-accent);
    background: var(--sc-accent-tint);
  }
</style>

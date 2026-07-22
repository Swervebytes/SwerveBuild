<script lang="ts">
  import StatusPill from "$lib/components/ui/StatusPill.svelte";
  import ProviderPicker from "$lib/components/providers/ProviderPicker.svelte";
  import ModelPicker from "$lib/components/models/ModelPicker.svelte";
  import MediaPicker from "$lib/components/models/MediaPicker.svelte";
  import { browserPane } from "$lib/stores/browserPane.svelte";
  import {
    type ChatUsage,
    formatTokens,
    hasKnownUsage,
    usagePercent,
    usageTone,
    usageTooltip,
  } from "$lib/chatUsage";

  let {
    title,
    projectName,
    projectPath,
    connected,
    activeSessionCount = 0,
    showModelPicker = false,
    modelId = null,
    modelSwitching = false,
    usage = null,
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
    /** ACP-reported context usage; null / incomplete → honest "—". */
    usage?: ChatUsage | null;
    onmodelchange?: (id: string | null) => void;
  } = $props();

  const known = $derived(hasKnownUsage(usage));
  const pct = $derived(usage ? usagePercent(usage) : null);
  const tone = $derived(usageTone(usage));
  const tip = $derived(usageTooltip(usage));
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
    <div
      class="usage"
      class:tone-ok={tone === "ok"}
      class:tone-warn={tone === "warn"}
      class:tone-high={tone === "high"}
      class:tone-critical={tone === "critical"}
      class:tone-unknown={tone === "unknown"}
      title={tip}
      data-testid="chat-usage"
      role="status"
      aria-label={known
        ? `Context usage ${formatTokens(usage!.used!)} of ${formatTokens(usage!.size!)}`
        : "Context usage unknown"}
    >
      {#if known && usage && pct != null}
        <span class="usage-nums"
          >{formatTokens(usage.used!)}&nbsp;/&nbsp;{formatTokens(usage.size!)}</span
        >
        <span class="usage-track" aria-hidden="true">
          <span class="usage-fill" style="width: {pct}%"></span>
        </span>
        <span class="usage-pct">{Math.round(pct)}%</span>
      {:else}
        <span class="usage-unknown" aria-hidden="true">—</span>
        <span class="usage-hint">ctx</span>
      {/if}
    </div>
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
    <!-- S16: image/video gen slot (Imagine remote today; local planned) -->
    <MediaPicker />
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

  /* Context / token usage (S14) — honest; "—" when agent has not reported. */
  .usage {
    display: inline-flex;
    align-items: center;
    gap: 0.35rem;
    min-height: 28px;
    padding: 0.15rem 0.45rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm, 6px);
    background: var(--bg-surface);
    font-family: var(--font-mono);
    font-size: 0.6875rem;
    color: var(--text-secondary);
    cursor: default;
    user-select: none;
    max-width: 11rem;
  }
  .usage-nums {
    white-space: nowrap;
    letter-spacing: -0.01em;
  }
  .usage-track {
    display: block;
    width: 2.25rem;
    height: 4px;
    border-radius: 99px;
    background: var(--border);
    overflow: hidden;
    flex-shrink: 0;
  }
  .usage-fill {
    display: block;
    height: 100%;
    border-radius: inherit;
    background: var(--sc-accent, #6cb5ff);
    transition: width 0.2s ease;
  }
  .usage-pct {
    font-variant-numeric: tabular-nums;
    min-width: 1.75rem;
    text-align: right;
  }
  .usage-unknown {
    font-size: 0.875rem;
    line-height: 1;
    color: var(--text-faint);
    padding: 0 0.1rem;
  }
  .usage-hint {
    font-size: 0.625rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    color: var(--text-faint);
  }
  .usage.tone-warn {
    color: var(--warning, #d4a017);
    border-color: color-mix(in srgb, var(--warning, #d4a017) 35%, var(--border));
  }
  .usage.tone-warn .usage-fill {
    background: var(--warning, #d4a017);
  }
  .usage.tone-high {
    color: #e07a2f;
    border-color: color-mix(in srgb, #e07a2f 40%, var(--border));
  }
  .usage.tone-high .usage-fill {
    background: #e07a2f;
  }
  .usage.tone-critical {
    color: var(--danger, #e25555);
    border-color: color-mix(in srgb, var(--danger, #e25555) 40%, var(--border));
  }
  .usage.tone-critical .usage-fill {
    background: var(--danger, #e25555);
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

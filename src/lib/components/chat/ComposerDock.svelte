<script lang="ts">
  import ModelsMenu from "$lib/components/models/ModelsMenu.svelte";
  import StatusPill from "$lib/components/ui/StatusPill.svelte";
  import {
    type ChatUsage,
    formatTokens,
    hasKnownUsage,
    hasKnownUsed,
    usagePercent,
    usageTone,
    usageTooltip,
  } from "$lib/chatUsage";

  let {
    showModelPicker = false,
    modelId = null,
    modelSwitching = false,
    usage = null,
    connected = false,
    onmodelchange,
  }: {
    showModelPicker?: boolean;
    modelId?: string | null;
    modelSwitching?: boolean;
    usage?: ChatUsage | null;
    /** This chat's agent session is ready for the next message. */
    connected?: boolean;
    onmodelchange?: (id: string | null) => void;
  } = $props();

  const known = $derived(hasKnownUsage(usage));
  const usedOnly = $derived(!known && hasKnownUsed(usage));
  const pct = $derived(usage ? usagePercent(usage) : null);
  const tone = $derived(usageTone(usage));
  const tip = $derived(usageTooltip(usage));
</script>

<div class="dock">
  <div class="dock-status">
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
      : usedOnly
        ? `Context usage ${formatTokens(usage!.used!)} tokens, window size unknown`
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
    {:else if usedOnly && usage}
      <span class="usage-nums" data-testid="chat-usage-used-only"
        >{formatTokens(usage.used!)}</span
      >
      <span class="usage-hint">ctx</span>
    {:else}
      <span class="usage-unknown" aria-hidden="true">–</span>
      <span class="usage-hint">ctx</span>
    {/if}
  </div>
    {#if connected}
      <StatusPill tone="success" label="Connected" />
    {:else}
      <StatusPill tone="warning" label="Connecting…" pulse />
    {/if}
  </div>

  <ModelsMenu
    sheetUp
    {showModelPicker}
    {modelId}
    {modelSwitching}
    {onmodelchange}
  />
</div>

<style>
  .dock {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem;
    margin-bottom: 0.45rem;
  }
  .dock-status {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    min-width: 0;
  }

  .usage {
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
    min-height: 28px;
    padding: 0.15rem 0.5rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm, 6px);
    background: var(--bg-surface);
    font-family: var(--font-mono);
    font-size: 0.6875rem;
    color: var(--text-secondary);
    cursor: default;
    user-select: none;
  }
  .usage-nums {
    white-space: nowrap;
    letter-spacing: -0.01em;
  }
  .usage-track {
    display: block;
    width: 3.25rem;
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
</style>

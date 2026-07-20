<script lang="ts">
  import Icon from "$lib/components/ui/Icon.svelte";
  import StatusPill from "$lib/components/ui/StatusPill.svelte";
  import type { WfRunRecord } from "$lib/workflows/api";
  import { fromEpoch, runStatusLabel, runStatusTone } from "$lib/workflows/model";

  let {
    runs,
    selected,
    onselect,
    oncancel,
    onrefresh,
  }: {
    runs: WfRunRecord[];
    selected: WfRunRecord | null;
    onselect: (run: WfRunRecord) => void;
    oncancel: (runId: string) => void;
    onrefresh: () => void;
  } = $props();

  let openNodes = $state<Record<string, boolean>>({});

  function durationLabel(run: WfRunRecord): string {
    if (!run.finished_at) return "";
    const secs = Number(run.finished_at) - Number(run.started_at);
    if (Number.isNaN(secs) || secs < 0) return "";
    return secs < 60 ? `${secs}s` : `${Math.round(secs / 60)}m`;
  }

  function pretty(value: unknown): string {
    try {
      return JSON.stringify(value, null, 2);
    } catch {
      return String(value);
    }
  }
</script>

<div class="runs">
  <div class="runs-head">
    <span class="mono-label">Run history</span>
    <button class="btn btn-ghost btn-icon" title="Refresh" onclick={onrefresh}>
      <Icon name="refresh" size={13} />
    </button>
  </div>

  {#if runs.length === 0}
    <p class="empty-note">No runs yet. Press Run to try the workflow.</p>
  {:else}
    <div class="run-list">
      {#each runs as run (run.id)}
        <button class="run-row" class:active={selected?.id === run.id} onclick={() => onselect(run)}>
          <StatusPill tone={runStatusTone(run.status)} label={runStatusLabel(run.status)}
            pulse={run.status === "running"} />
          <span class="run-when">{fromEpoch(run.started_at)}</span>
          <span class="run-meta">{run.trigger.kind}{durationLabel(run) ? ` · ${durationLabel(run)}` : ""}</span>
          {#if run.status === "running" || run.status === "queued"}
            <span class="stop" role="button" tabindex="0" title="Stop this run"
              onclick={(e) => { e.stopPropagation(); oncancel(run.id); }}
              onkeydown={(e) => { if (e.key === "Enter") { e.stopPropagation(); oncancel(run.id); } }}>
              <Icon name="stop" size={12} />
            </span>
          {/if}
        </button>
      {/each}
    </div>
  {/if}

  {#if selected}
    <div class="detail">
      <div class="mono-label">Nodes in this run</div>
      {#if selected.error}
        <div class="run-error">
          <Icon name="close" size={12} />
          <span>{selected.error.node_name ? `${selected.error.node_name}: ` : ""}{selected.error.message}</span>
        </div>
      {/if}
      {#each selected.nodes as n (n.node_id)}
        <div class="node-block">
          <button class="node-line" onclick={() => (openNodes[n.node_id] = !openNodes[n.node_id])}>
            <Icon name={openNodes[n.node_id] ? "chevron-down" : "chevron-right"} size={12} />
            <span class="node-name">{n.name}</span>
            <span class="node-stat" class:bad={n.status === "error"} class:dim={n.status === "skipped"}>
              {#if n.status === "success"}{n.items_in} in · {n.items_out} out · {n.duration_ms}ms
              {:else if n.status === "error"}failed
              {:else}{n.status}{/if}
            </span>
          </button>
          {#if openNodes[n.node_id]}
            {#if n.error}
              <div class="node-error">{n.error.message}</div>
            {/if}
            {#if selected.data[n.node_id]}
              {#each Object.entries(selected.data[n.node_id]) as [port, captured] (port)}
                <div class="port-cap">
                  <div class="port-cap-head">
                    {port} · {captured.total} item{captured.total === 1 ? "" : "s"}{captured.truncated ? " (sampled)" : ""}
                  </div>
                  <pre class="port-json">{pretty(captured.items)}</pre>
                </div>
              {/each}
            {:else}
              <div class="node-error dim-note">No captured data for this node.</div>
            {/if}
          {/if}
        </div>
      {/each}
    </div>
  {/if}
</div>

<style>
  .runs {
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
    padding: 0.9rem;
    overflow-y: auto;
  }

  .runs-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
  }

  .empty-note {
    font-size: 0.8125rem;
    color: var(--text-muted);
  }

  .run-list {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
  }

  .run-row {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.45rem 0.55rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg-muted);
    cursor: pointer;
    text-align: left;
    transition: border-color var(--dur-fast) var(--ease), background var(--dur-fast) var(--ease);
  }

  .run-row:hover {
    background: var(--bg-hover);
  }

  .run-row.active {
    border-color: var(--accent);
  }

  .run-when {
    font-size: 0.71875rem;
    color: var(--text-secondary);
    white-space: nowrap;
  }

  .run-meta {
    flex: 1;
    font-size: 0.6875rem;
    color: var(--text-faint);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .stop {
    display: grid;
    place-items: center;
    width: 22px;
    height: 22px;
    border-radius: 6px;
    color: var(--danger);
  }

  .stop:hover {
    background: var(--danger-tint);
  }

  .detail {
    display: flex;
    flex-direction: column;
    gap: 0.45rem;
    padding-top: 0.7rem;
    border-top: 1px solid var(--border);
  }

  .run-error {
    display: flex;
    align-items: flex-start;
    gap: 0.4rem;
    padding: 0.5rem 0.6rem;
    border-radius: var(--radius-sm);
    background: var(--danger-tint);
    color: var(--danger);
    font-size: 0.75rem;
    line-height: 1.4;
  }

  .node-block {
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg-muted);
    overflow: hidden;
  }

  .node-line {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    width: 100%;
    padding: 0.4rem 0.55rem;
    background: transparent;
    border: none;
    cursor: pointer;
    color: var(--text-secondary);
  }

  .node-line:hover {
    background: var(--bg-hover);
  }

  .node-name {
    flex: 1;
    font-size: 0.78125rem;
    font-weight: 600;
    text-align: left;
    color: var(--text-primary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .node-stat {
    font-size: 0.6875rem;
    color: var(--text-faint);
    white-space: nowrap;
  }

  .node-stat.bad {
    color: var(--danger);
  }

  .node-stat.dim {
    opacity: 0.7;
  }

  .node-error {
    padding: 0.4rem 0.6rem;
    font-size: 0.71875rem;
    color: var(--danger);
    border-top: 1px solid var(--border);
  }

  .dim-note {
    color: var(--text-faint);
  }

  .port-cap {
    border-top: 1px solid var(--border);
  }

  .port-cap-head {
    padding: 0.3rem 0.6rem;
    font-size: 0.65625rem;
    font-weight: 600;
    letter-spacing: 0.05em;
    text-transform: uppercase;
    color: var(--text-faint);
  }

  .port-json {
    margin: 0;
    padding: 0.4rem 0.6rem 0.6rem;
    max-height: 220px;
    overflow: auto;
    font-family: var(--font-mono);
    font-size: 0.6875rem;
    line-height: 1.45;
    color: var(--text-secondary);
    white-space: pre;
  }
</style>

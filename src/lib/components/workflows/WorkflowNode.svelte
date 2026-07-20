<script lang="ts">
  import { Handle, Position } from "@xyflow/svelte";
  import Icon from "$lib/components/ui/Icon.svelte";
  import type { CatalogEntry, WfNode, WfNodeRunSummary } from "$lib/workflows/api";
  import { categoryColor, nodeIcon } from "$lib/workflows/model";

  type RunOverlay = null | "running" | WfNodeRunSummary;

  let {
    data,
    selected = false,
  }: {
    data: { def: WfNode; spec: CatalogEntry; run: RunOverlay };
    selected?: boolean;
  } = $props();

  const spec = $derived(data.spec);
  const def = $derived(data.def);
  const color = $derived(categoryColor(spec.category));
  const outputs = $derived(
    def.on_error === "branch"
      ? [...spec.outputs, { name: "error", label: "Error" }]
      : spec.outputs,
  );
  const run = $derived(data.run);
  const runState = $derived(
    run === "running" ? "running" : run ? (run.status as string) : null,
  );

  function offset(i: number, n: number): string {
    return `${((i + 1) / (n + 1)) * 100}%`;
  }
</script>

<div
  class="wf-node"
  class:selected
  class:disabled={def.disabled}
  class:running={runState === "running"}
  class:ok={runState === "success"}
  class:failed={runState === "error"}
  class:skipped={runState === "skipped" || runState === "cancelled"}
  style="--node-color: {color}"
>
  {#each spec.inputs as port, i (port.name)}
    <Handle
      type="target"
      position={Position.Left}
      id={port.name}
      style={`top: ${offset(i, spec.inputs.length)}`}
    />
    {#if spec.inputs.length > 1}
      <span class="port-label in" style={`top: ${offset(i, spec.inputs.length)}`}>{port.label}</span>
    {/if}
  {/each}

  <div class="icon-chip">
    <Icon name={nodeIcon(spec.type)} size={15} />
  </div>
  <div class="body">
    <div class="name" title={def.name}>{def.name}</div>
    <div class="kind">{spec.label}</div>
  </div>

  {#if def.disabled}
    <span class="state-chip off">Off</span>
  {:else if runState === "running"}
    <span class="state-chip run"><span class="spinner"></span></span>
  {:else if run && run !== "running" && runState === "success"}
    <span class="state-chip ok"><Icon name="check" size={11} /> {run.items_out}</span>
  {:else if run && run !== "running" && runState === "error"}
    <span class="state-chip bad"><Icon name="close" size={11} /></span>
  {/if}

  {#each outputs as port, i (port.name)}
    <Handle
      type="source"
      position={Position.Right}
      id={port.name}
      class={port.name === "error" ? "error-port" : ""}
      style={`top: ${offset(i, outputs.length)}`}
    />
    {#if outputs.length > 1}
      <span class="port-label out" class:err={port.name === "error"} style={`top: ${offset(i, outputs.length)}`}>
        {port.label}
      </span>
    {/if}
  {/each}
</div>

<style>
  .wf-node {
    position: relative;
    display: flex;
    align-items: center;
    gap: 0.6rem;
    min-width: 178px;
    max-width: 240px;
    padding: 0.65rem 0.85rem 0.65rem 0.7rem;
    background: var(--bg-surface);
    border: 1px solid var(--border-strong);
    border-left: 3px solid var(--node-color);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow-sm);
    transition:
      box-shadow var(--dur-fast) var(--ease),
      border-color var(--dur-fast) var(--ease),
      opacity var(--dur-fast) var(--ease);
  }

  .wf-node.selected {
    border-color: var(--node-color);
    box-shadow:
      0 0 0 1.5px color-mix(in srgb, var(--node-color) 65%, transparent),
      var(--shadow-md);
  }

  .wf-node.disabled {
    opacity: 0.55;
    border-style: dashed;
  }

  .wf-node.running {
    border-color: var(--node-color);
    box-shadow: 0 0 14px -2px color-mix(in srgb, var(--node-color) 70%, transparent);
    animation: wf-pulse 1.4s ease-in-out infinite;
  }

  .wf-node.ok {
    border-color: color-mix(in srgb, var(--success) 70%, var(--border-strong));
  }

  .wf-node.failed {
    border-color: var(--danger);
    box-shadow: 0 0 12px -3px color-mix(in srgb, var(--danger) 70%, transparent);
  }

  .wf-node.skipped {
    opacity: 0.6;
  }

  @keyframes wf-pulse {
    0%,
    100% {
      box-shadow: 0 0 10px -3px color-mix(in srgb, var(--node-color) 60%, transparent);
    }
    50% {
      box-shadow: 0 0 18px -2px color-mix(in srgb, var(--node-color) 85%, transparent);
    }
  }

  .icon-chip {
    display: grid;
    place-items: center;
    width: 30px;
    height: 30px;
    flex: none;
    border-radius: var(--radius-sm);
    background: color-mix(in srgb, var(--node-color) 16%, transparent);
    color: var(--node-color);
  }

  .body {
    min-width: 0;
  }

  .name {
    font-size: 0.8125rem;
    font-weight: 600;
    color: var(--text-primary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .kind {
    font-size: 0.6875rem;
    color: var(--text-faint);
    white-space: nowrap;
  }

  .state-chip {
    position: absolute;
    top: -9px;
    right: 10px;
    display: inline-flex;
    align-items: center;
    gap: 0.2rem;
    padding: 0.1rem 0.4rem;
    border-radius: var(--radius-pill);
    font-size: 0.625rem;
    font-weight: 600;
    line-height: 1.3;
    border: 1px solid var(--border);
    background: var(--bg-surface-2);
    color: var(--text-muted);
  }

  .state-chip.ok {
    background: var(--success-tint);
    border-color: color-mix(in srgb, var(--success) 40%, transparent);
    color: var(--success);
  }

  .state-chip.bad {
    background: var(--danger-tint);
    border-color: color-mix(in srgb, var(--danger) 40%, transparent);
    color: var(--danger);
  }

  .state-chip.run {
    background: color-mix(in srgb, var(--node-color) 14%, var(--bg-surface));
    border-color: color-mix(in srgb, var(--node-color) 40%, transparent);
  }

  .spinner {
    width: 10px;
    height: 10px;
    border-radius: 50%;
    border: 2px solid color-mix(in srgb, var(--node-color) 30%, transparent);
    border-top-color: var(--node-color);
    animation: wf-spin 0.8s linear infinite;
  }

  @keyframes wf-spin {
    to {
      transform: rotate(360deg);
    }
  }

  .port-label {
    position: absolute;
    transform: translateY(-50%);
    font-size: 0.575rem;
    font-weight: 600;
    letter-spacing: 0.06em;
    text-transform: uppercase;
    color: var(--text-faint);
    pointer-events: none;
  }

  .port-label.in {
    left: 10px;
  }

  .port-label.out {
    right: 10px;
  }

  .port-label.err {
    color: var(--danger);
  }

  /* Handle (port dot) styling — override Svelte Flow defaults. */
  .wf-node :global(.svelte-flow__handle) {
    width: 9px;
    height: 9px;
    border-radius: 50%;
    background: var(--bg-surface-2);
    border: 2px solid var(--node-color);
    transition: transform var(--dur-fast) var(--ease);
  }

  .wf-node :global(.svelte-flow__handle:hover) {
    transform: scale(1.35);
  }

  .wf-node :global(.svelte-flow__handle.error-port) {
    border-color: var(--danger);
  }
</style>

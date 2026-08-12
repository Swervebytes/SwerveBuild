<script lang="ts">
  import Icon from "$lib/components/ui/Icon.svelte";
  import type { Automation, Project } from "$lib/types";
  import { triggerIcon } from "$lib/automations/model";

  let {
    automations,
    projects,
    onopen,
  }: {
    automations: Automation[];
    projects: Project[];
    onopen: (a: Automation) => void;
  } = $props();

  const NW = 184;
  const NH = 56;
  const COL_GAP = 250;
  const ROW_GAP = 74;
  const PAD_X = 36;
  const PAD_Y = 56;

  type Node = { a: Automation; x: number; y: number };
  type Frame = { name: string; x: number; y: number; w: number; h: number };

  const layout = $derived.by(() => {
    // Group automations into project columns (+ a "No project" bucket).
    const buckets: { name: string; items: Automation[] }[] = [];
    const byProject = new Map<string, Automation[]>();
    for (const a of automations) {
      const key = a.project_id ?? "__none__";
      if (!byProject.has(key)) byProject.set(key, []);
      byProject.get(key)!.push(a);
    }
    for (const p of projects) {
      const items = byProject.get(p.id);
      if (items && items.length) buckets.push({ name: p.name, items });
    }
    const orphans = byProject.get("__none__");
    if (orphans && orphans.length) buckets.push({ name: "No project", items: orphans });

    const nodes: Node[] = [];
    const frames: Frame[] = [];
    buckets.forEach((col, ci) => {
      const x = PAD_X + ci * COL_GAP;
      col.items.forEach((a, ri) => {
        nodes.push({ a, x, y: PAD_Y + ri * ROW_GAP });
      });
      frames.push({
        name: col.name,
        x: x - 14,
        y: PAD_Y - 30,
        w: NW + 28,
        h: col.items.length * ROW_GAP + 12,
      });
    });

    const byId = new Map(nodes.map((n) => [n.a.id, n]));
    const edges: { from: Node; to: Node }[] = [];
    for (const n of nodes) {
      const from = n.a.chain_input?.from;
      if (from && byId.has(from)) edges.push({ from: byId.get(from)!, to: n });
    }
    return { nodes, edges, frames };
  });

  function stripeColor(status: string | null | undefined): string {
    switch (status) {
      case "running":
      case "queued":
        return "var(--sc-accent)";
      case "success":
        return "var(--success)";
      case "error":
      case "launchfailed":
      case "timeout":
        return "var(--danger)";
      case "maxturns":
        return "var(--warning)";
      default:
        return "var(--border-strong)";
    }
  }

  function edgePath(from: Node, to: Node): string {
    const sx = from.x + NW;
    const sy = from.y + NH / 2;
    const tx = to.x + NW;
    const ty = to.y + NH / 2;
    const bow = Math.max(sx, tx) + 46;
    return `M ${sx} ${sy} C ${bow} ${sy} ${bow} ${ty} ${tx} ${ty}`;
  }

  // pan / zoom
  let svgEl: SVGSVGElement;
  let tx = $state(8);
  let ty = $state(6);
  let k = $state(0.95);
  let panning = $state(false);
  let last = { x: 0, y: 0 };

  function toSvg(cx: number, cy: number) {
    const pt = svgEl.createSVGPoint();
    pt.x = cx;
    pt.y = cy;
    return pt.matrixTransform(svgEl.getScreenCTM()!.inverse());
  }
  function onpointerdown(e: PointerEvent) {
    if ((e.target as Element).closest(".node")) return;
    panning = true;
    last = toSvg(e.clientX, e.clientY);
    svgEl.setPointerCapture(e.pointerId);
  }
  function onpointermove(e: PointerEvent) {
    if (!panning) return;
    const c = toSvg(e.clientX, e.clientY);
    tx += c.x - last.x;
    ty += c.y - last.y;
    last = c;
  }
  function onpointerup() {
    panning = false;
  }
  function onwheel(e: WheelEvent) {
    e.preventDefault();
    const s = toSvg(e.clientX, e.clientY);
    const wx = (s.x - tx) / k;
    const wy = (s.y - ty) / k;
    const f = e.deltaY < 0 ? 1.12 : 0.89;
    const nk = Math.max(0.45, Math.min(2.4, k * f));
    tx = s.x - wx * nk;
    ty = s.y - wy * nk;
    k = nk;
  }
  function reset() {
    tx = 8;
    ty = 6;
    k = 0.95;
  }
</script>

<div class="map-wrap">
  <svg
    bind:this={svgEl}
    class="runmap"
    class:grabbing={panning}
    viewBox="0 0 760 460"
    preserveAspectRatio="xMidYMid meet"
    onpointerdown={onpointerdown}
    onpointermove={onpointermove}
    onpointerup={onpointerup}
    onpointercancel={onpointerup}
    onwheel={onwheel}
    role="application"
    aria-label="Automation run-map"
  >
    <defs>
      <pattern id="rm-grid" width="26" height="26" patternUnits="userSpaceOnUse">
        <path d="M26 0H0V26" fill="none" stroke="var(--border)" stroke-width="0.6" opacity="0.5" />
      </pattern>
      <marker id="rm-arrow" viewBox="0 0 10 10" refX="9" refY="5" markerWidth="7" markerHeight="7" orient="auto-start-reverse">
        <path d="M0 0L10 5L0 10z" fill="var(--sc-accent)" opacity="0.7" />
      </marker>
    </defs>
    <rect x="-2000" y="-2000" width="6000" height="6000" fill="url(#rm-grid)" />
    <g transform="translate({tx},{ty}) scale({k})">
      {#each layout.frames as f (f.name + f.x)}
        <rect x={f.x} y={f.y} width={f.w} height={f.h} rx="14" class="frame" />
        <text x={f.x + 12} y={f.y + 18} class="frame-label">{f.name}</text>
      {/each}

      {#each layout.edges as e (e.from.a.id + e.to.a.id)}
        <path d={edgePath(e.from, e.to)} class="edge" marker-end="url(#rm-arrow)" />
      {/each}

      {#each layout.nodes as n (n.a.id)}
        <g
          class="node"
          transform="translate({n.x},{n.y})"
          onclick={() => onopen(n.a)}
          onkeydown={(e) => {
            if (e.key === "Enter" || e.key === " ") {
              e.preventDefault();
              onopen(n.a);
            }
          }}
          role="button"
          tabindex="0"
          aria-label="{n.a.name}: {n.a.enabled ? 'enabled' : 'disabled'}, {n.a.executor
            .mode} mode{n.a.state.last_status ? `, last run ${n.a.state.last_status}` : ''}"
        >
          <rect width={NW} height={NH} rx="11" class="node-card" />
          <rect width="5" height={NH} rx="2.5" fill={stripeColor(n.a.state.last_status)} />
          {#if n.a.state.last_status === "running"}
            <circle cx={NW - 15} cy="15" r="4" fill="var(--sc-accent)" class="pulse-dot" />
          {/if}
          <g transform="translate(16,15)" class="node-ico">
            <Icon name={triggerIcon(n.a.trigger)} size={17} />
          </g>
          <text x="42" y="23" class="node-title">{n.a.name.slice(0, 22)}</text>
          <text x="42" y="40" class="node-sub">{n.a.enabled ? "enabled" : "disabled"} · {n.a.executor.mode}</text>
          <rect width={NW} height={NH} rx="11" fill="transparent" />
        </g>
      {/each}
    </g>
  </svg>
  <div class="map-ctrls">
    <button type="button" onclick={reset} title="Reset view">Reset</button>
  </div>
</div>

<style>
  .map-wrap {
    position: relative;
    height: 460px;
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    overflow: hidden;
    background: var(--bg-muted);
  }
  .runmap {
    width: 100%;
    height: 100%;
    display: block;
    cursor: grab;
    touch-action: none;
  }
  .runmap.grabbing {
    cursor: grabbing;
  }
  .frame {
    fill: none;
    stroke: var(--border);
    stroke-dasharray: 3 4;
  }
  .frame-label {
    fill: var(--text-faint);
    font: 600 11px var(--font-mono);
    letter-spacing: 0.06em;
    text-transform: uppercase;
  }
  .edge {
    fill: none;
    stroke: var(--sc-accent);
    stroke-width: 1.6;
    opacity: 0.5;
  }
  .node {
    cursor: pointer;
  }
  .node-card {
    fill: var(--bg-surface);
    stroke: var(--border-strong);
    stroke-width: 1.2;
  }
  .node:hover .node-card {
    stroke: var(--sc-accent);
  }
  .node-ico {
    color: var(--text-muted);
  }
  .node-title {
    fill: var(--text-primary);
    font: 600 12.5px var(--font-sans);
  }
  .node-sub {
    fill: var(--text-faint);
    font: 11px var(--font-mono);
  }
  .pulse-dot {
    animation: rm-pulse 1.6s ease-in-out infinite;
  }
  @keyframes rm-pulse {
    0%,
    100% {
      opacity: 1;
    }
    50% {
      opacity: 0.3;
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .pulse-dot {
      animation: none;
    }
  }
  .map-ctrls {
    position: absolute;
    right: 12px;
    bottom: 12px;
  }
  .map-ctrls button {
    padding: 0.35rem 0.7rem;
    border: 1px solid var(--border);
    background: var(--bg-surface);
    color: var(--text-secondary);
    border-radius: var(--radius-sm);
    font-size: 0.75rem;
    cursor: pointer;
  }
  .map-ctrls button:hover {
    border-color: var(--sc-accent);
    color: var(--sc-accent);
  }
</style>

<script lang="ts">
  import { providerStore } from "$lib/stores/providers.svelte";
  import Icon from "$lib/components/ui/Icon.svelte";

  let {
    /** standalone = header trigger; panel = section inside Models sheet */
    variant = "standalone",
  }: {
    variant?: "standalone" | "panel";
  } = $props();

  let open = $state(false);
  let root: HTMLDivElement | undefined = $state();

  const providers = $derived(providerStore.all);
  const active = $derived(providerStore.active);
  const isPanel = $derived(variant === "panel");

  function toggle(e: MouseEvent) {
    e.stopPropagation();
    open = !open;
  }
  function close() {
    open = false;
  }

  async function choose(id: string, available: boolean) {
    if (!available) return;
    await providerStore.setActive(id);
    if (!isPanel) close();
  }

  function badgeFor(p: { available: boolean; kind: string }): string | null {
    if (p.available) return null;
    return p.kind === "http" ? "soon" : "not installed";
  }

  $effect(() => {
    if (isPanel || !open) return;
    const onDoc = (e: MouseEvent) => {
      if (root && !root.contains(e.target as Node)) close();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") close();
    };
    document.addEventListener("click", onDoc);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("click", onDoc);
      document.removeEventListener("keydown", onKey);
    };
  });
</script>

<div class="picker" class:panel={isPanel} bind:this={root}>
  {#if !isPanel}
    <button
      class="trigger"
      type="button"
      onclick={toggle}
      aria-haspopup="menu"
      aria-expanded={open}
      title="Switch provider"
    >
      <span class="swatch" style="--c: {active.accent}"></span>
      <span class="label">{active.label}</span>
      {#if active.model}<span class="model">{active.model}</span>{/if}
      <span class="chev" class:open><Icon name="chevron-down" size={13} /></span>
    </button>
  {/if}

  {#if isPanel || open}
    <div
      class="menu"
      class:menu-panel={isPanel}
      role={isPanel ? "group" : "menu"}
    >
      <div class="menu-head mono-label">Provider</div>
      {#each providers as p (p.id)}
        {@const badge = badgeFor(p)}
        <button
          class="row"
          class:current={p.active}
          type="button"
          role="menuitem"
          disabled={!p.available}
          onclick={() => choose(p.id, p.available)}
        >
          <span class="swatch" style="--c: {p.accent}"></span>
          <span class="row-label">{p.label}</span>
          <span class="row-id">{p.id}</span>
          {#if p.active}
            <span class="here">active</span>
          {:else if badge}
            <span class="soon">{badge}</span>
          {/if}
        </button>
      {/each}
    </div>
  {/if}
</div>

<style>
  .picker {
    position: relative;
    display: inline-flex;
  }
  .picker.panel {
    display: block;
    width: 100%;
    position: static;
  }

  .trigger {
    display: inline-flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.4rem 0.6rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg-surface-2);
    color: var(--text-primary);
    font-size: 0.8125rem;
    cursor: pointer;
    transition:
      background var(--dur-fast) var(--ease),
      border-color var(--dur-fast) var(--ease);
  }
  .trigger:hover {
    border-color: var(--border-strong);
  }

  .swatch {
    width: 9px;
    height: 9px;
    border-radius: 3px;
    background: var(--c, var(--sc-accent));
    box-shadow: var(--glow) var(--c, var(--sc-accent));
    flex: none;
  }
  .label {
    font-weight: 600;
  }
  .model {
    font-family: var(--font-mono);
    font-size: 0.6875rem;
    color: var(--text-muted);
  }
  .chev {
    display: inline-flex;
    color: var(--text-faint);
    transition: transform var(--dur) var(--ease);
  }
  .chev.open {
    transform: rotate(180deg);
  }

  .menu {
    position: absolute;
    top: calc(100% + 8px);
    right: 0;
    z-index: 60;
    min-width: 240px;
    padding: 0.35rem;
    background: var(--bg-surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow-lg);
    transform-origin: top right;
  }
  .menu.menu-panel {
    position: static;
    min-width: 0;
    width: 100%;
    padding: 0;
    border: 0;
    border-radius: 0;
    box-shadow: none;
    background: transparent;
  }
  .menu-head {
    padding: 0.5rem 0.6rem 0.4rem;
  }

  .row {
    display: grid;
    grid-template-columns: 12px 1fr auto auto;
    align-items: center;
    gap: 0.6rem;
    width: 100%;
    padding: 0.5rem 0.6rem;
    border: 0;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--text-primary);
    text-align: left;
    cursor: pointer;
    transition: background var(--dur-fast) var(--ease);
  }
  .row:hover:not(:disabled) {
    background: var(--bg-hover);
  }
  .row:disabled {
    cursor: not-allowed;
    opacity: 0.6;
  }
  .row.current {
    background: var(--sc-accent-tint);
  }
  .row-label {
    font-weight: 600;
    font-size: 0.8125rem;
  }
  .row.current .row-label {
    color: var(--sc-accent);
  }
  .row-id {
    font-family: var(--font-mono);
    font-size: 0.6875rem;
    color: var(--text-muted);
  }
  .here {
    font-family: var(--font-mono);
    font-size: 0.5625rem;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--sc-accent);
  }
  .soon {
    font-family: var(--font-mono);
    font-size: 0.5625rem;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--text-muted);
    background: var(--bg-muted);
    border-radius: var(--radius-pill);
    padding: 0.15rem 0.45rem;
  }
</style>

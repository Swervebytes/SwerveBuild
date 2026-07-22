<script lang="ts">
  import { scale } from "svelte/transition";
  import { cubicOut } from "svelte/easing";
  import { invoke } from "@tauri-apps/api/core";
  import type { ModelInfo } from "$lib/types";
  import Icon from "$lib/components/ui/Icon.svelte";

  let {
    value = null,
    disabled = false,
    onchange,
    /** standalone = header trigger; panel = section inside Models sheet */
    variant = "standalone",
  }: {
    /** Selected model id; null = the agent's own default. */
    value?: string | null;
    disabled?: boolean;
    onchange: (id: string | null) => void;
    variant?: "standalone" | "panel";
  } = $props();

  let open = $state(false);
  let root: HTMLDivElement | undefined = $state();
  let models = $state<ModelInfo[]>([]);
  let loaded = $state(false);

  const triggerLabel = $derived(value ?? "default");
  const isPanel = $derived(variant === "panel");

  async function loadModels() {
    try {
      models = await invoke<ModelInfo[]>("list_models");
    } catch {
      models = [];
    }
    loaded = true;
  }

  function toggle(e: MouseEvent) {
    e.stopPropagation();
    if (disabled) return;
    open = !open;
    if (open && !loaded) loadModels();
  }
  function close() {
    open = false;
  }

  function choose(id: string | null) {
    if (!isPanel) close();
    if (id === (value ?? null)) return;
    onchange(id);
  }

  $effect(() => {
    if (isPanel && !loaded) void loadModels();
  });

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
      disabled={disabled}
      title="Agent (chat) model — text + tool decisions, not image pixels"
    >
      <Icon name="settings" size={12} />
      <span class="label mono">{disabled ? "switching…" : `agent · ${triggerLabel}`}</span>
      <span class="chev" class:open><Icon name="chevron-down" size={13} /></span>
    </button>
  {/if}

  {#if isPanel || open}
    <div
      class="menu"
      class:menu-panel={isPanel}
      role={isPanel ? "group" : "menu"}
      transition:scale={{ duration: isPanel ? 0 : 140, start: 0.97, opacity: 0, easing: cubicOut }}
    >
      <div class="menu-head mono-label">Agent model</div>
      <p class="honesty">
        Runs chat and decides tools. Does not render images — use the image section for that.
      </p>
      <button class="row" class:current={value === null} type="button" role="menuitem" onclick={() => choose(null)}>
        <span class="row-label">Default</span>
        <span class="row-note">agent's own model</span>
        {#if value === null}<span class="here">active</span>{/if}
      </button>
      {#if !loaded}
        <div class="empty">Loading models…</div>
      {:else if models.length === 0}
        <div class="empty">No models found — is Grok installed?</div>
      {:else}
        {#each models as m (m.id)}
          <button
            class="row"
            class:current={value === m.id}
            type="button"
            role="menuitem"
            onclick={() => choose(m.id)}
          >
            <span class="row-label" class:mono={m.kind !== "endpoint"}>{m.label}</span>
            <span class="row-note">
              {#if m.kind === "endpoint"}{m.note}{:else if m.is_default}default{:else if m.note}{m.note}{/if}
            </span>
            {#if value === m.id}<span class="here">active</span>{/if}
          </button>
        {/each}
      {/if}
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
    gap: 0.4rem;
    padding: 0.4rem 0.6rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg-surface-2);
    color: var(--text-secondary);
    font-size: 0.75rem;
    cursor: pointer;
    transition:
      background var(--dur-fast) var(--ease),
      border-color var(--dur-fast) var(--ease);
  }
  .trigger:hover:not(:disabled) {
    border-color: var(--border-strong);
    color: var(--text-primary);
  }
  .trigger:disabled {
    cursor: default;
    opacity: 0.7;
  }

  .mono {
    font-family: var(--font-mono);
  }
  .label {
    max-width: 140px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .chev {
    display: inline-flex;
    color: var(--text-faint);
    transition: transform var(--dur) var(--ease);
  }
  .chev.open {
    transform: rotate(180deg);
  }

  .honesty {
    margin: 0 0.45rem 0.35rem;
    font-size: 0.6875rem;
    line-height: 1.4;
    color: var(--text-muted);
  }
  .menu {
    position: absolute;
    top: calc(100% + 8px);
    right: 0;
    z-index: 60;
    min-width: 260px;
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
    grid-template-columns: 1fr auto auto;
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
  .row:hover {
    background: var(--bg-hover);
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
  .row-note {
    font-family: var(--font-mono);
    font-size: 0.6563rem;
    color: var(--text-muted);
    max-width: 150px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .here {
    font-family: var(--font-mono);
    font-size: 0.5625rem;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--sc-accent);
  }
  .empty {
    padding: 0.6rem;
    font-size: 0.75rem;
    color: var(--text-muted);
  }
</style>

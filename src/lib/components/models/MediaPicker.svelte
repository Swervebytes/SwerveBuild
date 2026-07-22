<script lang="ts">
  import { scale } from "svelte/transition";
  import { cubicOut } from "svelte/easing";
  import { invoke } from "@tauri-apps/api/core";
  import type { MediaProviderInfo, MediaProvidersView } from "$lib/types";
  import Icon from "$lib/components/ui/Icon.svelte";

  /** Workspace image-provider slot (S16). Separate from chat/agent model. */

  let open = $state(false);
  let root: HTMLDivElement | undefined = $state();
  let view = $state<MediaProvidersView | null>(null);
  let busy = $state(false);
  let error = $state<string | null>(null);

  const selected = $derived(
    view?.imageProviders.find((p) => p.id === view?.selectedImageProviderId) ?? null,
  );
  const triggerLabel = $derived(
    selected
      ? selected.locality === "remote"
        ? `img · ${shortLabel(selected)}`
        : `img · ${shortLabel(selected)}`
      : "img · …",
  );

  function shortLabel(p: MediaProviderInfo): string {
    if (p.id === "imagine") return "Imagine";
    if (p.id === "local") return "Local";
    return p.label;
  }

  async function load() {
    try {
      view = await invoke<MediaProvidersView>("list_media_providers");
      error = null;
    } catch (e) {
      error = String(e);
      view = null;
    }
  }

  function toggle(e: MouseEvent) {
    e.stopPropagation();
    open = !open;
    if (open) void load();
  }
  function close() {
    open = false;
  }

  async function choose(p: MediaProviderInfo) {
    if (!p.available) return;
    if (p.id === view?.selectedImageProviderId) {
      close();
      return;
    }
    busy = true;
    error = null;
    try {
      view = await invoke<MediaProvidersView>("set_image_provider", { id: p.id });
      close();
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }

  $effect(() => {
    void load();
  });

  $effect(() => {
    if (!open) return;
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

<div class="picker" bind:this={root} data-testid="media-picker">
  <button
    class="trigger"
    type="button"
    onclick={toggle}
    aria-haspopup="menu"
    aria-expanded={open}
    disabled={busy}
    title={selected
      ? `Image gen: ${selected.label} (${selected.locality}) — ${selected.note}`
      : "Image generation provider"}
  >
    <Icon name="image" size={12} />
    <span class="label mono">{busy ? "…" : triggerLabel}</span>
    <span class="chev" class:open><Icon name="chevron-down" size={13} /></span>
  </button>

  {#if open}
    <div
      class="menu"
      role="menu"
      transition:scale={{ duration: 140, start: 0.97, opacity: 0, easing: cubicOut }}
    >
      <div class="menu-head mono-label">Image generation</div>
      <p class="honesty">
        Chat model only decides tools. Pixels come from the provider below — not from local Qwen /
        Grok chat weights.
      </p>
      {#if error}
        <div class="err">{error}</div>
      {/if}
      {#if !view}
        <div class="empty">Loading…</div>
      {:else}
        {#each view.imageProviders as p (p.id)}
          <button
            class="row"
            class:current={p.id === view.selectedImageProviderId}
            class:disabled={!p.available}
            type="button"
            role="menuitem"
            disabled={!p.available || busy}
            onclick={() => choose(p)}
          >
            <span class="row-main">
              <span class="row-label">{p.label}</span>
              <span class="row-note">{p.note}</span>
            </span>
            <span class="tag" class:remote={p.locality === "remote"} class:local={p.locality === "local"}
              >{p.locality}</span
            >
            {#if p.id === view.selectedImageProviderId}<span class="here">active</span>{/if}
          </button>
        {/each}
        <div class="menu-head mono-label video-head">Video generation</div>
        {#each view.videoProviders as p (p.id)}
          <div class="row static" class:current={p.id === view.selectedVideoProviderId}>
            <span class="row-main">
              <span class="row-label">{p.label}</span>
              <span class="row-note">{p.note}</span>
            </span>
            <span class="tag" class:remote={p.locality === "remote"} class:local={p.locality === "local"}
              >{p.locality}</span
            >
          </div>
        {/each}
      {/if}
    </div>
  {/if}
</div>

<style>
  .picker {
    position: relative;
  }
  .trigger {
    display: inline-flex;
    align-items: center;
    gap: 0.3rem;
    max-width: 9.5rem;
    height: 28px;
    padding: 0 0.45rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm, 6px);
    background: var(--bg-surface);
    color: var(--text-secondary);
    font-size: 0.6875rem;
    cursor: pointer;
  }
  .trigger:hover:not(:disabled) {
    border-color: var(--sc-accent);
    color: var(--text-primary);
  }
  .trigger:disabled {
    opacity: 0.55;
    cursor: wait;
  }
  .label {
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .chev {
    display: grid;
    transition: transform 0.15s ease;
  }
  .chev.open {
    transform: rotate(180deg);
  }
  .menu {
    position: absolute;
    top: calc(100% + 4px);
    right: 0;
    z-index: 40;
    width: min(22rem, 92vw);
    padding: 0.4rem;
    border: 1px solid var(--border);
    border-radius: var(--radius, 8px);
    background: var(--bg-surface);
    box-shadow: var(--shadow-md);
  }
  .menu-head {
    padding: 0.35rem 0.45rem 0.2rem;
    color: var(--text-faint);
  }
  .video-head {
    margin-top: 0.35rem;
    border-top: 1px solid var(--border);
    padding-top: 0.45rem;
  }
  .honesty {
    margin: 0 0.45rem 0.4rem;
    font-size: 0.6875rem;
    line-height: 1.4;
    color: var(--text-muted);
  }
  .row {
    display: flex;
    align-items: flex-start;
    gap: 0.4rem;
    width: 100%;
    text-align: left;
    padding: 0.4rem 0.45rem;
    border: 0;
    border-radius: 6px;
    background: transparent;
    color: var(--text-primary);
    cursor: pointer;
  }
  .row:hover:not(:disabled):not(.static) {
    background: var(--bg-muted);
  }
  .row.static {
    cursor: default;
    opacity: 0.9;
  }
  .row.disabled,
  .row:disabled {
    opacity: 0.55;
    cursor: not-allowed;
  }
  .row.current {
    background: color-mix(in srgb, var(--sc-accent) 10%, transparent);
  }
  .row-main {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 0.15rem;
  }
  .row-label {
    font-size: 0.8125rem;
    font-weight: 600;
  }
  .row-note {
    font-size: 0.6875rem;
    color: var(--text-muted);
    line-height: 1.35;
  }
  .tag {
    flex: none;
    font-size: 0.625rem;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    padding: 0.1rem 0.3rem;
    border-radius: 4px;
    border: 1px solid var(--border);
    color: var(--text-faint);
  }
  .tag.remote {
    color: var(--sc-accent);
    border-color: color-mix(in srgb, var(--sc-accent) 35%, var(--border));
  }
  .tag.local {
    color: var(--text-secondary);
  }
  .here {
    flex: none;
    font-size: 0.625rem;
    color: var(--sc-accent);
    text-transform: uppercase;
  }
  .empty,
  .err {
    padding: 0.5rem 0.45rem;
    font-size: 0.75rem;
    color: var(--text-muted);
  }
  .err {
    color: var(--danger);
  }
</style>

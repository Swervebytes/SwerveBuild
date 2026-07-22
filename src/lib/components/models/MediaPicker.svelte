<script lang="ts">
  import { scale } from "svelte/transition";
  import { cubicOut } from "svelte/easing";
  import { invoke } from "@tauri-apps/api/core";
  import type { MediaProviderInfo, MediaProvidersView } from "$lib/types";
  import Icon from "$lib/components/ui/Icon.svelte";

  /** Workspace image-provider slot (S16). Separate from chat/agent model. */

  let {
    /** standalone = header trigger; panel = section inside Models sheet */
    variant = "standalone",
    /** Fired after successful load/set so parent summary can refresh */
    onchange,
  }: {
    variant?: "standalone" | "panel";
    onchange?: (view: MediaProvidersView | null) => void;
  } = $props();

  let open = $state(false);
  let root: HTMLDivElement | undefined = $state();
  let view = $state<MediaProvidersView | null>(null);
  let busy = $state(false);
  let error = $state<string | null>(null);
  let comfyUrl = $state("http://127.0.0.1:8188");
  let genPrompt = $state("");
  let genBusy = $state(false);

  const isPanel = $derived(variant === "panel");
  const selected = $derived(
    view?.imageProviders.find((p) => p.id === view?.selectedImageProviderId) ?? null,
  );
  const triggerLabel = $derived(
    selected ? `img · ${shortLabel(selected)}` : "img · …",
  );

  function shortLabel(p: MediaProviderInfo): string {
    if (p.id === "imagine") return "Imagine";
    if (p.id === "local") return p.available === false ? "Local (off)" : "Local";
    return p.label;
  }

  async function load() {
    try {
      view = await invoke<MediaProvidersView>("list_media_providers");
      error = null;
      if (view?.localImage?.baseUrl) comfyUrl = view.localImage.baseUrl;
      onchange?.(view);
    } catch (e) {
      error = String(e);
      view = null;
      onchange?.(null);
    }
  }

  async function saveComfyUrl() {
    busy = true;
    error = null;
    try {
      view = await invoke<MediaProvidersView>("set_comfy_base_url", { url: comfyUrl });
      onchange?.(view);
    } catch (e) {
      error = String(e);
    } finally {
      busy = false;
    }
  }

  async function testLocalGen() {
    if (!genPrompt.trim()) {
      error = "Enter a short test prompt";
      return;
    }
    genBusy = true;
    error = null;
    try {
      const path = await invoke<string>("generate_local_image", {
        prompt: genPrompt.trim(),
        width: 512,
        height: 512,
      });
      error = null;
      genPrompt = "";
      error = `Saved: ${path}`;
      await load();
    } catch (e) {
      error = String(e);
    } finally {
      genBusy = false;
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
      if (!isPanel) close();
      return;
    }
    busy = true;
    error = null;
    try {
      view = await invoke<MediaProvidersView>("set_image_provider", { id: p.id });
      onchange?.(view);
      if (!isPanel) close();
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

<div class="picker" class:panel={isPanel} bind:this={root} data-testid="media-picker">
  {#if !isPanel}
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
  {/if}

  {#if isPanel || open}
    <div
      class="menu"
      class:menu-panel={isPanel}
      role={isPanel ? "group" : "menu"}
      transition:scale={{ duration: isPanel ? 0 : 140, start: 0.97, opacity: 0, easing: cubicOut }}
    >
      <div class="menu-head mono-label">Image generation</div>
      <p class="honesty">
        Chat model only decides tools. Pixels come from the provider below — not from local Qwen /
        Grok chat weights. Local uses ComfyUI; install checkpoints in Comfy Manager (not GGUF catalog).
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

        <div class="menu-head mono-label video-head">ComfyUI (local)</div>
        <div class="comfy-block">
          <input
            class="comfy-input mono"
            type="text"
            spellcheck="false"
            bind:value={comfyUrl}
            placeholder="http://127.0.0.1:8188"
            aria-label="ComfyUI base URL"
          />
          <button class="btn-mini" type="button" disabled={busy} onclick={saveComfyUrl}>Save URL</button>
          <button class="btn-mini" type="button" disabled={busy} onclick={() => load()}>Probe</button>
        </div>
        {#if view.localImage}
          <p class="comfy-status mono-label">
            {view.localImage.reachable ? "Up" : "Down"} · {view.localImage.baseUrl}
            {#if view.localImage.checkpoints?.length}
              · {view.localImage.checkpoints.length} ckpt
            {/if}
          </p>
        {/if}
        {#if view.localImage?.reachable}
          <div class="comfy-block">
            <input
              class="comfy-input"
              type="text"
              bind:value={genPrompt}
              placeholder="Test prompt…"
              aria-label="Local generate test prompt"
            />
            <button class="btn-mini" type="button" disabled={genBusy} onclick={testLocalGen}>
              {genBusy ? "…" : "Gen"}
            </button>
          </div>
        {/if}

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
  .picker.panel {
    display: block;
    width: 100%;
    position: static;
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
  .mono {
    font-family: var(--font-mono);
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
  .menu.menu-panel {
    position: static;
    width: 100%;
    padding: 0;
    border: 0;
    border-radius: 0;
    box-shadow: none;
    background: transparent;
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
    word-break: break-word;
  }
  .comfy-block {
    display: flex;
    flex-wrap: wrap;
    gap: 0.35rem;
    padding: 0.25rem 0.45rem 0.45rem;
    align-items: center;
  }
  .comfy-input {
    flex: 1;
    min-width: 8rem;
    font-size: 0.75rem;
    padding: 0.3rem 0.4rem;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg-muted);
    color: var(--text-primary);
  }
  .btn-mini {
    font-size: 0.6875rem;
    padding: 0.3rem 0.45rem;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: var(--bg-surface);
    color: var(--text-secondary);
    cursor: pointer;
  }
  .btn-mini:hover:not(:disabled) {
    border-color: var(--sc-accent);
    color: var(--text-primary);
  }
  .btn-mini:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .comfy-status {
    padding: 0 0.45rem 0.35rem;
    color: var(--text-muted);
    font-size: 0.625rem;
  }
</style>

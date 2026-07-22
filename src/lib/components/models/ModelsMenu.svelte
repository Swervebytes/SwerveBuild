<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import Icon from "$lib/components/ui/Icon.svelte";
  import ProviderPicker from "$lib/components/providers/ProviderPicker.svelte";
  import ModelPicker from "$lib/components/models/ModelPicker.svelte";
  import MediaPicker from "$lib/components/models/MediaPicker.svelte";
  import { providerStore } from "$lib/stores/providers.svelte";
  import type { MediaProvidersView } from "$lib/types";

  /**
   * S19 — one chat-header control for provider + agent model + image slot.
   * Perf: prefs-only summary on mount (no Comfy probe); live probe when sheet opens.
   */

  let {
    showModelPicker = false,
    modelId = null,
    modelSwitching = false,
    onmodelchange,
  }: {
    showModelPicker?: boolean;
    modelId?: string | null;
    modelSwitching?: boolean;
    onmodelchange?: (id: string | null) => void;
  } = $props();

  let open = $state(false);
  let root: HTMLDivElement | undefined = $state();
  let mediaView = $state<MediaProvidersView | null>(null);
  let sheetReady = $state(false);

  const providerLabel = $derived(providerStore.active.label);
  const agentLabel = $derived(
    showModelPicker ? (modelSwitching ? "…" : (modelId ?? "default")) : null,
  );
  const imgLabel = $derived.by(() => {
    const id = mediaView?.selectedImageProviderId;
    if (!id) return "…";
    if (id === "imagine") return "Imagine";
    if (id === "local") {
      const p = mediaView?.imageProviders.find((x) => x.id === "local");
      // prefs-only marks local available; live view may show off
      if (p && p.available === false) return "Local off";
      return "Local";
    }
    return id;
  });

  const summary = $derived(
    [providerLabel, agentLabel ? `agent ${agentLabel}` : null, `img ${imgLabel}`]
      .filter(Boolean)
      .join(" · "),
  );

  /** Header label only — never blocks chat paint on Comfy. */
  async function loadPrefsSummary() {
    try {
      mediaView = await invoke<MediaProvidersView>("list_media_providers", {
        refresh: false,
        prefsOnly: true,
      });
    } catch {
      /* leave previous */
    }
  }

  function toggle(e: MouseEvent) {
    e.stopPropagation();
    open = !open;
    if (open) {
      // Mount heavy panel content on next frame so the button state flips first.
      requestAnimationFrame(() => {
        sheetReady = true;
      });
    } else {
      sheetReady = false;
    }
  }
  function close() {
    open = false;
    sheetReady = false;
  }

  $effect(() => {
    void loadPrefsSummary();
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

<div class="models" bind:this={root} data-testid="models-menu">
  <button
    class="trigger"
    type="button"
    onclick={toggle}
    aria-haspopup="dialog"
    aria-expanded={open}
    title="Provider, agent model, and image generation"
  >
    <span class="swatch" style="--c: {providerStore.active.accent}"></span>
    <span class="trigger-text">
      <span class="trigger-title">Models</span>
      <span class="trigger-sum mono">{summary}</span>
    </span>
    <span class="chev" class:open><Icon name="chevron-down" size={13} /></span>
  </button>

  {#if open}
    <div class="sheet" role="dialog" aria-label="Models">
      <div class="sheet-head">
        <span class="mono-label">Models</span>
        <p class="sheet-hint">
          Provider runs the agent. Agent model is chat/tools only. Image slot owns pixels.
        </p>
      </div>

      {#if sheetReady}
        <section class="section">
          <ProviderPicker variant="panel" />
        </section>

        {#if showModelPicker && onmodelchange}
          <section class="section section-border">
            <ModelPicker
              variant="panel"
              value={modelId}
              disabled={modelSwitching}
              onchange={onmodelchange}
            />
          </section>
        {/if}

        <section class="section section-border">
          <MediaPicker
            variant="panel"
            onchange={(v) => {
              mediaView = v;
            }}
          />
        </section>
      {:else}
        <p class="sheet-loading mono-label">Loading…</p>
      {/if}
    </div>
  {/if}
</div>

<style>
  .models {
    position: relative;
    display: inline-flex;
  }

  .trigger {
    display: inline-flex;
    align-items: center;
    gap: 0.45rem;
    max-width: min(16rem, 42vw);
    min-height: 30px;
    padding: 0.25rem 0.5rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm, 6px);
    background: var(--bg-surface);
    color: var(--text-primary);
    cursor: pointer;
    transition:
      border-color var(--dur-fast) var(--ease),
      background var(--dur-fast) var(--ease);
  }
  .trigger:hover {
    border-color: var(--sc-accent);
  }
  .swatch {
    width: 9px;
    height: 9px;
    border-radius: 3px;
    background: var(--c, var(--sc-accent));
    box-shadow: var(--glow) var(--c, var(--sc-accent));
    flex: none;
  }
  .trigger-text {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    min-width: 0;
    gap: 0.05rem;
  }
  .trigger-title {
    font-size: 0.6875rem;
    font-weight: 700;
    letter-spacing: 0.02em;
    line-height: 1.1;
  }
  .trigger-sum {
    font-size: 0.625rem;
    color: var(--text-muted);
    max-width: 13rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .mono {
    font-family: var(--font-mono);
  }
  .chev {
    display: inline-flex;
    color: var(--text-faint);
    flex: none;
    transition: transform 0.12s ease;
  }
  .chev.open {
    transform: rotate(180deg);
  }

  .sheet {
    position: absolute;
    top: calc(100% + 6px);
    right: 0;
    z-index: 70;
    width: min(22rem, 92vw);
    max-height: min(70vh, 36rem);
    overflow: auto;
    padding: 0.45rem 0.4rem 0.55rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-lg, 10px);
    background: var(--bg-surface);
    box-shadow: var(--shadow-lg);
  }
  .sheet-head {
    padding: 0.35rem 0.5rem 0.55rem;
  }
  .sheet-hint {
    margin: 0.25rem 0 0;
    font-size: 0.6875rem;
    line-height: 1.4;
    color: var(--text-muted);
  }
  .sheet-loading {
    padding: 0.75rem 0.5rem;
    color: var(--text-faint);
  }
  .section {
    padding: 0.15rem 0;
  }
  .section-border {
    border-top: 1px solid var(--border);
    margin-top: 0.25rem;
    padding-top: 0.35rem;
  }
</style>

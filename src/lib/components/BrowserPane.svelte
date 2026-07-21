<script lang="ts">
  import { onMount } from "svelte";
  import { browserPane } from "$lib/stores/browserPane.svelte";
  import { permissionStore } from "$lib/stores/permissions.svelte";

  // The reserved area the NATIVE child webview is positioned over. It stays
  // empty on purpose — Tauri paints the webview above it.
  let viewport: HTMLDivElement;
  let urlValue = $state("");

  // Reflect the landed URL back into the address bar (unless the user is typing).
  let editing = $state(false);
  $effect(() => {
    if (!editing) urlValue = browserPane.url;
  });

  function pushBounds() {
    if (!viewport) return;
    const r = viewport.getBoundingClientRect();
    if (r.width < 1 || r.height < 1) return;
    // A native child webview is positioned in DEVICE pixels; getBoundingClientRect
    // is CSS px. Convert with the current DPR (this display runs at fractional
    // scale). On a DPI change the window `resize` event re-runs this.
    const d = window.devicePixelRatio || 1;
    void browserPane.setBounds(
      Math.round(r.left * d),
      Math.round(r.top * d),
      Math.round(r.width * d),
      Math.round(r.height * d),
    );
  }

  // A native webview always paints ABOVE the HTML, so while a modal is up we
  // park it offscreen, then realign once the modal closes.
  $effect(() => {
    const modalOpen = !!permissionStore.current;
    if (modalOpen) browserPane.park();
    else pushBounds();
  });

  onMount(() => {
    // Align on mount + whenever our reserved box or the window changes size.
    requestAnimationFrame(pushBounds);
    const ro = new ResizeObserver(() => pushBounds());
    ro.observe(viewport);
    const onResize = () => pushBounds();
    window.addEventListener("resize", onResize);
    return () => {
      ro.disconnect();
      window.removeEventListener("resize", onResize);
      // Leaving the chat (or closing) unmounts us — park the native webview so it
      // doesn't float over non-chat pages.
      browserPane.park();
    };
  });

  function submitUrl(e: SubmitEvent) {
    e.preventDefault();
    editing = false;
    void browserPane.navigate(urlValue);
  }
</script>

<section class="browser-pane" aria-label="Local preview browser">
  <div class="toolbar">
    <button
      class="tb-btn"
      type="button"
      title="Back"
      aria-label="Back"
      onclick={() => browserPane.back()}
    >
      <svg viewBox="0 0 16 16" width="15" height="15" aria-hidden="true"
        ><path d="M10 3 5 8l5 5" /></svg
      >
    </button>
    <button
      class="tb-btn"
      type="button"
      title="Forward"
      aria-label="Forward"
      onclick={() => browserPane.forward()}
    >
      <svg viewBox="0 0 16 16" width="15" height="15" aria-hidden="true"
        ><path d="M6 3l5 5-5 5" /></svg
      >
    </button>
    <button
      class="tb-btn"
      type="button"
      title="Reload"
      aria-label="Reload"
      onclick={() => browserPane.reload()}
    >
      <svg viewBox="0 0 16 16" width="15" height="15" aria-hidden="true"
        ><path d="M13 8a5 5 0 1 1-1.6-3.7M13 3v2.5h-2.5" /></svg
      >
    </button>
    <form class="url-form" onsubmit={submitUrl}>
      <input
        class="url-input"
        type="text"
        spellcheck="false"
        autocomplete="off"
        placeholder="Type a URL — localhost or a website"
        bind:value={urlValue}
        onfocus={() => (editing = true)}
        onblur={() => (editing = false)}
      />
    </form>
    {#if browserPane.loading}
      <span class="tb-spin" aria-label="Loading">
        <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"
          ><path d="M8 2a6 6 0 1 1-6 6" /></svg
        >
      </span>
    {/if}
    <button
      class="tb-btn close"
      type="button"
      title="Close browser"
      aria-label="Close browser"
      onclick={() => browserPane.closePane()}
    >
      <svg viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"
        ><path d="M4 4l8 8M12 4l-8 8" /></svg
      >
    </button>
  </div>

  {#if browserPane.message}
    <div class="pane-msg" role="status">{browserPane.message}</div>
  {/if}

  <div class="viewport" bind:this={viewport}>
    <p class="hint">Preview browser · type a URL above to load a page</p>
  </div>
</section>

<style>
  .browser-pane {
    display: flex;
    flex-direction: column;
    width: clamp(420px, 44%, 960px);
    min-width: 380px;
    height: 100%;
    border-left: 1px solid var(--border);
    background: var(--bg-app);
    overflow: hidden;
  }

  .toolbar {
    display: flex;
    align-items: center;
    gap: 4px;
    height: 44px;
    flex-shrink: 0;
    padding: 0 8px;
    background: var(--bg-surface);
    border-bottom: 1px solid var(--border);
  }

  .tb-btn {
    display: grid;
    place-items: center;
    width: 30px;
    height: 30px;
    flex-shrink: 0;
    border: 1px solid transparent;
    border-radius: var(--radius-sm, 6px);
    background: transparent;
    color: var(--text-secondary);
    cursor: pointer;
    transition:
      background var(--dur-fast) var(--ease),
      color var(--dur-fast) var(--ease);
  }
  .tb-btn svg {
    stroke: currentColor;
    stroke-width: 1.5;
    fill: none;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  .tb-btn:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }
  .tb-btn.close:hover {
    background: #e8452c;
    color: #fff;
  }

  .url-form {
    flex: 1;
    min-width: 0;
    display: flex;
  }
  .url-input {
    flex: 1;
    min-width: 0;
    height: 30px;
    padding: 0 10px;
    border: 1px solid var(--border);
    border-radius: var(--radius-pill, 8px);
    background: var(--bg-app);
    color: var(--text-primary);
    font-size: 0.8125rem;
  }
  .url-input:focus {
    outline: none;
    border-color: var(--sc-accent);
  }

  .tb-spin {
    display: grid;
    place-items: center;
    color: var(--sc-accent);
    animation: spin 0.8s linear infinite;
  }
  .tb-spin svg {
    stroke: currentColor;
    stroke-width: 1.6;
    fill: none;
    stroke-linecap: round;
  }
  @keyframes spin {
    to {
      transform: rotate(360deg);
    }
  }
  @media (prefers-reduced-motion: reduce) {
    .tb-spin {
      animation: none;
    }
  }

  .pane-msg {
    padding: 4px 12px;
    font-size: 0.75rem;
    color: #e8452c;
    background: var(--bg-surface);
    border-bottom: 1px solid var(--border);
  }

  /* Reserved box the native webview is positioned over. The hint only shows
     through before the webview is aligned or if it fails to attach. */
  .viewport {
    flex: 1;
    min-height: 0;
    position: relative;
    background: var(--bg-app);
    display: grid;
    place-items: center;
  }
  .hint {
    margin: 0;
    padding: 0 1rem;
    text-align: center;
    color: var(--text-muted);
    font-size: 0.8125rem;
  }
</style>

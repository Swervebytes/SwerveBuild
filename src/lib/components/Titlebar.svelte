<script lang="ts">
  import { onMount } from "svelte";
  import { getCurrentWindow, type Window } from "@tauri-apps/api/window";

  let maximized = $state(false);

  // Acquire the window lazily so the component still renders outside a Tauri
  // runtime (e.g. a plain browser preview), where getCurrentWindow() throws.
  function appWindow(): Window | null {
    try {
      return getCurrentWindow();
    } catch {
      return null;
    }
  }

  async function refreshMax() {
    try {
      maximized = (await appWindow()?.isMaximized()) ?? false;
    } catch {
      /* ignore */
    }
  }

  function minimize() {
    appWindow()?.minimize().catch(() => {});
  }
  function toggleMaximize() {
    appWindow()?.toggleMaximize().catch(() => {});
  }
  function close() {
    appWindow()?.close().catch(() => {});
  }

  onMount(() => {
    refreshMax();
    let cancelled = false;
    let unlisten: (() => void) | undefined;
    appWindow()
      ?.onResized(() => refreshMax())
      .then((fn) => {
        // Unmount may beat this resolve; detach immediately if so, else remember.
        if (cancelled) fn();
        else unlisten = fn;
      })
      .catch(() => {});
    return () => {
      cancelled = true;
      unlisten?.();
    };
  });
</script>

<div class="titlebar">
  <div class="pane rail" data-tauri-drag-region></div>
  <div class="pane main" data-tauri-drag-region>
    <div class="controls">
      <button class="ctl" type="button" onclick={minimize} aria-label="Minimize" title="Minimize">
        <svg viewBox="0 0 10 10" width="10" height="10" aria-hidden="true">
          <line x1="1" y1="5" x2="9" y2="5" />
        </svg>
      </button>
      <button
        class="ctl"
        type="button"
        onclick={toggleMaximize}
        aria-label={maximized ? "Restore" : "Maximize"}
        title={maximized ? "Restore" : "Maximize"}
      >
        {#if maximized}
          <svg viewBox="0 0 10 10" width="10" height="10" aria-hidden="true">
            <rect x="1" y="2.5" width="6" height="6" rx="0.5" />
            <path d="M3 2.5V1.5h5.5V7H7.5" fill="none" />
          </svg>
        {:else}
          <svg viewBox="0 0 10 10" width="10" height="10" aria-hidden="true">
            <rect x="1" y="1" width="8" height="8" rx="0.5" />
          </svg>
        {/if}
      </button>
      <button class="ctl close" type="button" onclick={close} aria-label="Close" title="Close">
        <svg viewBox="0 0 10 10" width="10" height="10" aria-hidden="true">
          <line x1="1" y1="1" x2="9" y2="9" />
          <line x1="9" y1="1" x2="1" y2="9" />
        </svg>
      </button>
    </div>
  </div>
</div>

<style>
  .titlebar {
    display: flex;
    height: var(--titlebar-height);
    flex-shrink: 0;
    user-select: none;
  }
  .pane {
    height: 100%;
  }
  .rail {
    width: var(--sidebar-width);
    background: var(--bg-sidebar);
    border-right: 1px solid var(--border);
  }
  .main {
    flex: 1;
    background: var(--bg-app);
    display: flex;
    align-items: stretch;
    justify-content: flex-end;
    min-width: 0;
  }

  .controls {
    display: flex;
    height: 100%;
  }
  .ctl {
    display: grid;
    place-items: center;
    width: 46px;
    height: 100%;
    border: 0;
    background: transparent;
    color: var(--text-muted);
    cursor: pointer;
    transition:
      background var(--dur-fast) var(--ease),
      color var(--dur-fast) var(--ease);
  }
  .ctl svg {
    stroke: currentColor;
    stroke-width: 1.2;
    fill: none;
    stroke-linecap: round;
  }
  .ctl:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }
  .ctl.close:hover {
    background: #e8452c;
    color: #fff;
  }
</style>

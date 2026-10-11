<script lang="ts">
  import { browserPane } from "$lib/stores/browserPane.svelte";

  let {
    title,
    projectName,
    projectPath,
    activeSessionCount = 0,
    onexport,
  }: {
    title: string;
    projectName?: string;
    projectPath?: string;
    activeSessionCount?: number;
    /** P4.3: export this chat as Markdown; resolves to the saved path. */
    onexport?: () => Promise<string>;
  } = $props();

  let exportState = $state<"idle" | "busy" | "done" | "error">("idle");
  let exportTitle = $state("Export chat as Markdown");

  async function runExport() {
    if (!onexport || exportState === "busy") return;
    exportState = "busy";
    try {
      const path = await onexport();
      exportState = "done";
      exportTitle = `Saved: ${path}`;
    } catch (e) {
      exportState = "error";
      exportTitle = String(e);
    }
    setTimeout(() => (exportState = "idle"), 4000);
  }

</script>

<header class="chat-header">
  <div class="titles">
    <h1 class="title">{title}</h1>
    <p class="subtitle">
      {#if projectName}
        <span class="proj">{projectName}</span>
        {#if projectPath}<span class="path">{projectPath}</span>{/if}
      {:else}
        Loading project…
      {/if}
    </p>
  </div>

  <div class="actions">
    {#if activeSessionCount > 0}
      <span class="badge badge-muted" title="Live chats. A fourth chat pauses the oldest.">{activeSessionCount}/3</span>
    {/if}
    <button
      class="browser-toggle"
      class:active={browserPane.open}
      type="button"
      onclick={() => browserPane.toggle()}
      aria-label="Toggle preview browser"
      aria-pressed={browserPane.open}
      title="Preview browser: open a localhost or web page here"
    >
      <svg viewBox="0 0 16 16" width="15" height="15" aria-hidden="true">
        <circle cx="8" cy="8" r="6.25" />
        <path d="M1.75 8h12.5M8 1.75c1.9 2 1.9 10.5 0 12.5M8 1.75c-1.9 2-1.9 10.5 0 12.5" />
      </svg>
    </button>
    {#if onexport}
      <button
        class="btn btn-ghost btn-sm"
        type="button"
        data-testid="chat-export-md"
        title={exportTitle}
        aria-label="Export chat as Markdown"
        disabled={exportState === "busy"}
        onclick={runExport}
      >
        {#if exportState === "done"}Saved ✓{:else if exportState === "error"}Export failed{:else}Export{/if}
      </button>
    {/if}
    <a class="btn btn-ghost btn-sm" href="/projects">All chats</a>
  </div>
</header>

<style>
  .chat-header {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 1rem;
    padding-bottom: 0.9rem;
    margin-bottom: 0.25rem;
    border-bottom: 1px solid var(--border);
    flex-shrink: 0;
  }
  .titles {
    min-width: 0;
  }
  .title {
    font-size: 1.125rem;
    font-weight: 700;
    letter-spacing: -0.01em;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .subtitle {
    margin-top: 0.2rem;
    display: flex;
    gap: 0.5rem;
    align-items: baseline;
    font-size: 0.8125rem;
    color: var(--text-secondary);
    min-width: 0;
  }
  .path {
    font-family: var(--font-mono);
    font-size: 0.75rem;
    color: var(--text-faint);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .actions {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-shrink: 0;
    flex-wrap: wrap;
    justify-content: flex-end;
  }

  .browser-toggle {
    display: grid;
    place-items: center;
    width: 30px;
    height: 30px;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm, 6px);
    background: var(--bg-surface);
    color: var(--text-secondary);
    cursor: pointer;
    transition:
      background var(--dur-fast) var(--ease),
      color var(--dur-fast) var(--ease),
      border-color var(--dur-fast) var(--ease);
  }
  .browser-toggle svg {
    stroke: currentColor;
    stroke-width: 1.3;
    fill: none;
    stroke-linecap: round;
    stroke-linejoin: round;
  }
  .browser-toggle:hover {
    color: var(--text-primary);
    border-color: var(--sc-accent);
  }
  .browser-toggle.active {
    color: var(--sc-accent);
    border-color: var(--sc-accent);
    background: var(--sc-accent-tint);
  }
</style>

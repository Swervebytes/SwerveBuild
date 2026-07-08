<script lang="ts">
  import StatusPill from "$lib/components/ui/StatusPill.svelte";
  import ProviderPicker from "$lib/components/providers/ProviderPicker.svelte";

  let {
    title,
    projectName,
    projectPath,
    connected,
    activeSessionCount = 0,
  }: {
    title: string;
    projectName?: string;
    projectPath?: string;
    connected: boolean;
    activeSessionCount?: number;
  } = $props();
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
    {#if connected}
      <StatusPill tone="success" label="Connected" />
    {:else}
      <StatusPill tone="warning" label="Connecting…" pulse />
    {/if}
    {#if activeSessionCount > 0}
      <span class="badge badge-muted">{activeSessionCount}/3</span>
    {/if}
    <ProviderPicker />
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
  }
</style>

<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import Icon from "$lib/components/ui/Icon.svelte";

  type CommandResult = {
    success: boolean;
    message: string;
  };

  let content = $state("");
  let savedContent = $state("");
  let dirty = $state(false);
  let saving = $state(false);
  let loading = $state(true);
  let status = $state<string | null>(null);

  onMount(async () => {
    try {
      content = await invoke<string>("read_memory");
      savedContent = content;
    } catch (error) {
      status = String(error);
    } finally {
      loading = false;
    }
  });

  function onInput(event: Event) {
    const target = event.target as HTMLTextAreaElement;
    content = target.value;
    dirty = content !== savedContent;
  }

  async function save() {
    saving = true;
    status = null;
    try {
      const result = await invoke<CommandResult>("write_memory", { content });
      if (result.success) {
        savedContent = content;
        dirty = false;
        status = `Saved to ${result.message}`;
      } else {
        status = result.message;
      }
    } catch (error) {
      status = String(error);
    } finally {
      saving = false;
    }
  }
</script>

<div class="page">
  <header class="page-header">
    <div class="header-row">
      <div>
        <h1 class="page-title">Memories</h1>
        <p class="page-subtitle">
          <span class="badge badge-muted"><Icon name="agent" size={12} /> Grok</span>
          Global memory at <span class="mono">~/.grok/memory/MEMORY.md</span>
        </p>
      </div>
      <div class="header-actions">
        {#if dirty}
          <span class="badge badge-warning">Unsaved</span>
        {/if}
        <button class="btn btn-primary" type="button" onclick={save} disabled={!dirty || saving}>
          <Icon name="check" size={15} />
          {saving ? "Saving…" : "Save"}
        </button>
      </div>
    </div>
  </header>

  <section class="editor card">
    {#if loading}
      <p class="placeholder muted">Loading memory…</p>
    {:else}
      <textarea
        class="editor-input"
        placeholder="# Memory&#10;&#10;Add persistent context for Grok Build here…"
        value={content}
        oninput={onInput}
        spellcheck="false"
      ></textarea>
    {/if}
  </section>

  {#if status}
    <p class="status">{status}</p>
  {/if}
</div>

<style>
  .page {
    display: flex;
    flex-direction: column;
    height: calc(100vh - var(--titlebar-height) - 3rem);
    max-width: 860px;
    margin-inline: auto;
  }
  .page-header {
    margin-bottom: 1rem;
    flex-shrink: 0;
  }
  .header-row {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 1rem;
  }
  .page-subtitle {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: wrap;
  }
  .badge :global(svg) {
    margin-right: 0.15rem;
  }
  .mono {
    font-family: var(--font-mono);
    font-size: 0.8125rem;
    color: var(--text-faint);
  }
  .header-actions {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }

  .editor {
    flex: 1;
    display: flex;
    padding: 0;
    overflow: hidden;
    min-height: 0;
  }
  .editor-input {
    flex: 1;
    width: 100%;
    border: none;
    outline: none;
    resize: none;
    padding: 1rem 1.25rem;
    background: transparent;
    color: var(--text-primary);
    font-family: var(--font-mono);
    font-size: 0.8125rem;
    line-height: 1.65;
  }
  .placeholder {
    padding: 1rem 1.25rem;
  }
  .muted {
    color: var(--text-muted);
  }
  .status {
    margin-top: 0.75rem;
    font-size: 0.8125rem;
    color: var(--text-secondary);
    flex-shrink: 0;
  }
</style>

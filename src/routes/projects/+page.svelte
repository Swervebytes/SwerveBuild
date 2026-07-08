<script lang="ts">
  import { onMount } from "svelte";
  import { goto } from "$app/navigation";
  import { invoke } from "@tauri-apps/api/core";
  import { createChat, pickProjectFolder } from "$lib/workspace";
  import { workspaceStore } from "$lib/stores/workspace.svelte";
  import Icon from "$lib/components/ui/Icon.svelte";
  import EmptyState from "$lib/components/ui/EmptyState.svelte";

  let loading = $state(true);
  let busy = $state<string | null>(null);
  let expanded = $state<Record<string, boolean>>({});

  const projects = $derived(workspaceStore.projects);
  const activeCount = $derived(workspaceStore.activeSessions.length);

  async function refresh() {
    await workspaceStore.refresh();
    const next = { ...expanded };
    for (const project of workspaceStore.projects) {
      next[project.id] ??= true;
    }
    expanded = next;
  }

  onMount(async () => {
    await refresh();
    loading = false;
  });

  async function addProject() {
    busy = "project";
    try {
      const project = await pickProjectFolder();
      if (project) {
        await refresh();
        expanded[project.id] = true;
      }
    } finally {
      busy = null;
    }
  }

  async function startChat(projectId: string) {
    busy = projectId;
    try {
      const chat = await createChat(projectId);
      await workspaceStore.refresh();
      await goto(`/chat/${chat.id}`);
    } finally {
      busy = null;
    }
  }

  async function deleteProject(projectId: string) {
    if (!confirm("Delete this project and all its chats?")) return;
    await invoke("remove_project", { projectId });
    await refresh();
  }

  async function deleteChat(chatId: string) {
    if (!confirm("Delete this chat?")) return;
    await invoke("remove_chat", { chatId });
    await refresh();
  }

  function toggleProject(projectId: string) {
    expanded[projectId] = !expanded[projectId];
  }

  function formatTime(value: string) {
    return new Date(Number(value) * 1000).toLocaleString();
  }
</script>

<div class="page">
  <header class="page-header">
    <div class="header-row">
      <div>
        <h1 class="page-title">Projects</h1>
        <p class="page-subtitle">
          Organize repos and their chats
          {#if activeCount > 0}· {activeCount}/3 connected in background{/if}
        </p>
      </div>
      <button class="btn btn-primary" type="button" disabled={busy !== null} onclick={addProject}>
        <Icon name="plus" size={15} />
        {busy === "project" ? "Opening…" : "Add Project"}
      </button>
    </div>
  </header>

  {#if loading}
    <p class="muted">Loading projects…</p>
  {:else if projects.length === 0}
    <section class="card">
      <EmptyState
        icon="folder"
        title="No projects yet"
        description="Add a repo folder to give Grok its working context, then start a chat."
      >
        <button class="btn btn-primary" type="button" onclick={addProject}>
          <Icon name="plus" size={15} /> Add Project
        </button>
      </EmptyState>
    </section>
  {:else}
    <div class="project-list">
      {#each projects as project (project.id)}
        {@const chats = workspaceStore.chatsForProject(project.id)}
        <section class="card project-card">
          <div class="project-header">
            <button class="project-toggle" type="button" onclick={() => toggleProject(project.id)}>
              <span class="chevron" class:open={expanded[project.id]}>
                <Icon name="chevron-right" size={16} />
              </span>
              <span class="project-icon"><Icon name="folder" size={16} /></span>
              <div class="project-meta">
                <h2>{project.name}</h2>
                <p class="path">{project.path}</p>
              </div>
            </button>
            <div class="project-actions">
              <button
                class="btn btn-accent btn-sm"
                type="button"
                disabled={busy !== null}
                onclick={() => startChat(project.id)}
              >
                <Icon name="plus" size={14} /> New Chat
              </button>
              <button
                class="btn btn-icon"
                type="button"
                title="Delete project"
                onclick={() => deleteProject(project.id)}
              >
                <Icon name="trash" size={15} />
              </button>
            </div>
          </div>

          {#if expanded[project.id]}
            <div class="chat-list">
              {#if chats.length === 0}
                <p class="muted small">No chats yet for this project.</p>
              {:else}
                {#each chats as chat (chat.id)}
                  <div class="chat-row">
                    <a class="chat-open" href="/chat/{chat.id}">
                      <span class="chat-title">
                        {#if workspaceStore.isActive(chat.id)}
                          <span class="live" title="Connected"></span>
                        {/if}
                        {chat.title}
                      </span>
                      <span class="chat-time">{formatTime(chat.updated_at)}</span>
                    </a>
                    <button
                      class="btn btn-icon"
                      type="button"
                      title="Delete chat"
                      onclick={() => deleteChat(chat.id)}
                    >
                      <Icon name="trash" size={14} />
                    </button>
                  </div>
                {/each}
              {/if}
            </div>
          {/if}
        </section>
      {/each}
    </div>
  {/if}
</div>

<style>
  .page {
    max-width: 860px;
    margin-inline: auto;
  }
  .page-header {
    margin-bottom: 1.25rem;
  }
  .header-row {
    display: flex;
    justify-content: space-between;
    align-items: flex-start;
    gap: 1rem;
  }

  .project-list {
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
  }
  .project-card {
    padding: 0;
    overflow: hidden;
  }
  .project-header {
    display: flex;
    justify-content: space-between;
    gap: 1rem;
    padding: 0.9rem 1rem;
  }
  .project-toggle {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    background: none;
    border: none;
    color: inherit;
    cursor: pointer;
    text-align: left;
    padding: 0;
    flex: 1;
    min-width: 0;
  }
  .chevron {
    display: inline-flex;
    color: var(--text-muted);
    transition: transform var(--dur) var(--ease);
    flex: none;
  }
  .chevron.open {
    transform: rotate(90deg);
  }
  .project-icon {
    display: inline-flex;
    color: var(--sc-accent);
    flex: none;
  }
  .project-meta {
    min-width: 0;
  }
  .project-meta h2 {
    font-size: 0.9375rem;
    font-weight: 600;
    margin-bottom: 0.15rem;
  }
  .path {
    font-family: var(--font-mono);
    font-size: 0.75rem;
    color: var(--text-faint);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .project-actions {
    display: flex;
    gap: 0.4rem;
    flex-shrink: 0;
    align-items: flex-start;
  }

  .chat-list {
    padding: 0.5rem 1rem 0.9rem;
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
    border-top: 1px solid var(--border);
  }
  .chat-row {
    display: flex;
    align-items: center;
    gap: 0.4rem;
  }
  .chat-open {
    flex: 1;
    display: flex;
    justify-content: space-between;
    gap: 1rem;
    align-items: center;
    padding: 0.55rem 0.7rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg-muted);
    transition:
      background var(--dur-fast) var(--ease),
      border-color var(--dur-fast) var(--ease);
    min-width: 0;
  }
  .chat-open:hover {
    background: var(--bg-hover);
    border-color: var(--border-strong);
  }
  .chat-title {
    font-size: 0.875rem;
    font-weight: 500;
    display: inline-flex;
    align-items: center;
    gap: 0.45rem;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .live {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--success);
    box-shadow: var(--glow) var(--success);
    flex: none;
  }
  .chat-time {
    font-size: 0.75rem;
    color: var(--text-faint);
    flex-shrink: 0;
  }
  .muted {
    color: var(--text-muted);
  }
  .small {
    font-size: 0.8125rem;
  }
</style>

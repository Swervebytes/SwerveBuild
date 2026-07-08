<script lang="ts">
  import { page } from "$app/stores";
  import { goto } from "$app/navigation";
  import { navItems } from "$lib/nav";
  import { createChat } from "$lib/workspace";
  import { workspaceStore } from "$lib/stores/workspace.svelte";
  import { providerStore } from "$lib/stores/providers.svelte";
  import Wordmark from "./Wordmark.svelte";
  import Icon from "./ui/Icon.svelte";

  const appVersion = "0.1.0";
  let creating = $state(false);

  const recent = $derived(workspaceStore.recent(7));

  function isActive(href: string): boolean {
    const path = $page.url.pathname;
    if (href === "/") return path === "/";
    return path === href || path.startsWith(`${href}/`);
  }

  async function newChat() {
    if (creating) return;
    const projects = workspaceStore.projects;
    if (projects.length === 1) {
      creating = true;
      try {
        const chat = await createChat(projects[0].id);
        await workspaceStore.refresh();
        await goto(`/chat/${chat.id}`);
      } finally {
        creating = false;
      }
    } else {
      // zero or many projects — let the user pick / add one
      await goto("/projects");
    }
  }
</script>

<aside class="sidebar">
  <header class="brand">
    <a href="/" class="brand-link" aria-label="Home">
      <Wordmark size="md" />
    </a>
  </header>

  <button class="new-chat" type="button" onclick={newChat} disabled={creating}>
    <Icon name="plus" size={16} stroke={2.4} />
    <span>New chat</span>
  </button>

  <nav class="nav" aria-label="Main navigation">
    {#each navItems as item}
      <a
        href={item.href}
        class="nav-item"
        class:active={isActive(item.href)}
        aria-current={isActive(item.href) ? "page" : undefined}
      >
        <span class="nav-icon"><Icon name={item.icon} size={17} /></span>
        <span class="nav-label">{item.label}</span>
      </a>
    {/each}
  </nav>

  <div class="recent">
    <div class="recent-head mono-label">Recent</div>
    {#if recent.length === 0}
      <p class="recent-empty">No chats yet</p>
    {:else}
      <div class="recent-list">
        {#each recent as chat (chat.id)}
          <a
            href="/chat/{chat.id}"
            class="recent-item"
            class:active={$page.url.pathname === `/chat/${chat.id}`}
          >
            <span class="recent-title">{chat.title}</span>
            {#if workspaceStore.isActive(chat.id)}
              <span class="live" title="Connected"></span>
            {/if}
          </a>
        {/each}
      </div>
    {/if}
  </div>

  <footer class="sidebar-footer">
    <div class="provider" title="Active provider">
      <span class="swatch" style="--c: {providerStore.active.accent}"></span>
      <span class="provider-label">{providerStore.active.label}</span>
    </div>
    <span class="version">v{appVersion}</span>
  </footer>
</aside>

<style>
  .sidebar {
    display: flex;
    flex-direction: column;
    width: var(--sidebar-width);
    min-width: var(--sidebar-width);
    height: 100%;
    background: var(--bg-sidebar);
    border-right: 1px solid var(--border);
  }

  .brand {
    padding: 1.1rem 1.15rem 0.75rem;
  }
  .brand-link {
    display: inline-flex;
  }

  .new-chat {
    display: flex;
    align-items: center;
    gap: 0.55rem;
    margin: 0 0.75rem 0.5rem;
    padding: 0.6rem 0.8rem;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--bg-surface);
    color: var(--text-primary);
    font-weight: 600;
    font-size: 0.875rem;
    cursor: pointer;
    transition:
      background var(--dur-fast) var(--ease),
      border-color var(--dur-fast) var(--ease),
      box-shadow var(--dur-fast) var(--ease);
  }
  .new-chat:hover:not(:disabled) {
    border-color: var(--sc-accent);
    box-shadow: var(--glow) var(--sc-accent-tint);
  }
  .new-chat :global(svg) {
    color: var(--sc-accent);
  }
  .new-chat:disabled {
    opacity: 0.6;
    cursor: not-allowed;
  }

  .nav {
    display: flex;
    flex-direction: column;
    gap: 0.1rem;
    padding: 0.35rem 0.625rem 0.5rem;
  }

  .nav-item {
    display: flex;
    align-items: center;
    gap: 0.7rem;
    padding: 0.5rem 0.7rem;
    border-radius: var(--radius-sm);
    color: var(--text-secondary);
    transition:
      background var(--dur-fast) var(--ease),
      color var(--dur-fast) var(--ease);
  }
  .nav-item:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }
  .nav-item.active {
    background: var(--bg-active);
    color: var(--text-primary);
    font-weight: 500;
  }
  .nav-item.active .nav-icon {
    color: var(--sc-accent);
  }
  .nav-icon {
    display: inline-flex;
    color: var(--text-muted);
    transition: color var(--dur-fast) var(--ease);
  }
  .nav-item:hover .nav-icon {
    color: var(--text-secondary);
  }
  .nav-label {
    font-size: 0.875rem;
  }

  .recent {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 0.5rem 0.625rem 0.5rem;
    border-top: 1px solid var(--border);
    margin-top: 0.25rem;
  }
  .recent-head {
    padding: 0.5rem 0.7rem 0.4rem;
  }
  .recent-empty {
    padding: 0.25rem 0.7rem;
    font-size: 0.8125rem;
    color: var(--text-faint);
  }
  .recent-list {
    display: flex;
    flex-direction: column;
    gap: 0.05rem;
  }
  .recent-item {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    padding: 0.45rem 0.7rem;
    border-radius: var(--radius-sm);
    color: var(--text-secondary);
    transition:
      background var(--dur-fast) var(--ease),
      color var(--dur-fast) var(--ease);
  }
  .recent-item:hover {
    background: var(--bg-hover);
    color: var(--text-primary);
  }
  .recent-item.active {
    background: var(--bg-active);
    color: var(--text-primary);
  }
  .recent-title {
    flex: 1;
    font-size: 0.8125rem;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .live {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--success);
    box-shadow: var(--glow) var(--success);
    flex: none;
  }

  .sidebar-footer {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.5rem;
    padding: 0.7rem 1rem 0.85rem;
    border-top: 1px solid var(--border);
  }
  .provider {
    display: inline-flex;
    align-items: center;
    gap: 0.45rem;
    min-width: 0;
  }
  .swatch {
    width: 9px;
    height: 9px;
    border-radius: 3px;
    background: var(--c, var(--sc-accent));
    box-shadow: var(--glow) var(--c, var(--sc-accent));
    flex: none;
  }
  .provider-label {
    font-size: 0.75rem;
    color: var(--text-secondary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .version {
    font-size: 0.6875rem;
    color: var(--text-faint);
    font-family: var(--font-mono);
    flex: none;
  }
</style>

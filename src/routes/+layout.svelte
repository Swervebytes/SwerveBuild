<script lang="ts">
  import "../app.css";
  import { onMount } from "svelte";
  import { page } from "$app/stores";
  import { invoke } from "@tauri-apps/api/core";
  import { openUrl } from "@tauri-apps/plugin-opener";
  import Sidebar from "$lib/components/Sidebar.svelte";
  import Titlebar from "$lib/components/Titlebar.svelte";
  import BrowserPane from "$lib/components/BrowserPane.svelte";
  import Icon from "$lib/components/ui/Icon.svelte";
  import { browserPane } from "$lib/stores/browserPane.svelte";
  import { theme } from "$lib/stores/theme.svelte";
  import { workspaceStore } from "$lib/stores/workspace.svelte";
  import { providerStore } from "$lib/stores/providers.svelte";
  import { permissionStore } from "$lib/stores/permissions.svelte";
  import PermissionModal from "$lib/components/chat/PermissionModal.svelte";

  const FEEDBACK_URL = "https://feedback.roaringbytes.com/?project=swervebuild";

  let { children } = $props();

  // Hide the feedback button on the chat page, where its bottom-right corner
  // would compete with the message composer at narrow widths.
  const showFeedback = $derived(!$page.url.pathname.startsWith("/chat/"));

  // The preview browser is a CHAT tool — only render its docked pane on a chat
  // route. Leaving chat unmounts BrowserPane, which parks the native webview.
  const isChatRoute = $derived($page.url.pathname.startsWith("/chat/"));

  // Publish route/title for MCP app_ui_state (no CDP required).
  $effect(() => {
    const route = $page.url.pathname;
    const title =
      typeof document !== "undefined" && document.title
        ? document.title
        : "Swerve Build";
    const permissionModalOpen = !!permissionStore.current;
    void invoke("publish_app_ui_state", {
      state: {
        route,
        title,
        permissionModalOpen,
        updatedAt: new Date().toISOString(),
      },
    }).catch(() => {
      /* browser / pre-window */
    });
  });

  onMount(() => {
    theme.init();
    workspaceStore.refresh();
    providerStore.load();
    // App-level tool-approval listener, so a background chat's permission
    // request surfaces on any page — not only when the chat route is mounted.
    permissionStore.start();
  });

  async function openFeedback() {
    try {
      await openUrl(FEEDBACK_URL);
    } catch {
      // Opener unavailable (e.g. browser dev mode) — fall back to a new tab.
      window.open(FEEDBACK_URL, "_blank", "noopener");
    }
  }
</script>

<div class="app-shell">
  <Titlebar />
  <div class="body">
    <Sidebar />
    <main class="content">
      {@render children()}
    </main>
    {#if browserPane.open && isChatRoute}
      <BrowserPane />
    {/if}
  </div>
</div>

{#if showFeedback}
  <button
    class="feedback-fab"
    type="button"
    title="Send feedback"
    aria-label="Send feedback"
    onclick={openFeedback}
  >
    <Icon name="chat" size={15} />
    <span>Feedback</span>
  </button>
{/if}

{#if permissionStore.current}
  <PermissionModal
    request={permissionStore.current}
    queueLength={permissionStore.queueLength}
    isBackground={permissionStore.current.chatId !== $page.params.id}
    onrespond={(o) => permissionStore.respond(o)}
  />
{/if}

<style>
  .app-shell {
    display: flex;
    flex-direction: column;
    height: 100vh;
    overflow: hidden;
  }

  .body {
    display: flex;
    flex: 1;
    min-height: 0;
    overflow: hidden;
  }

  .content {
    flex: 1;
    overflow: auto;
    padding: 1.5rem 2rem;
    background: var(--bg-app);
  }

  .feedback-fab {
    position: fixed;
    right: 1rem;
    bottom: 1rem;
    z-index: 40;
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
    padding: 0.45rem 0.75rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-pill);
    background: var(--bg-surface);
    color: var(--text-secondary);
    font-size: 0.8125rem;
    font-weight: 500;
    cursor: pointer;
    box-shadow: var(--shadow-md);
    opacity: 0.85;
    transition:
      opacity var(--dur-fast) var(--ease),
      border-color var(--dur-fast) var(--ease),
      color var(--dur-fast) var(--ease),
      transform var(--dur-fast) var(--ease);
  }
  .feedback-fab:hover {
    opacity: 1;
    color: var(--text-primary);
    border-color: var(--sc-accent);
    transform: translateY(-1px);
  }
  .feedback-fab :global(svg) {
    color: var(--sc-accent);
  }
  @media (prefers-reduced-motion: reduce) {
    .feedback-fab {
      transition: none;
    }
  }
</style>

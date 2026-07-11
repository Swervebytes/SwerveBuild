<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import Wordmark from "$lib/components/Wordmark.svelte";
  import Icon from "$lib/components/ui/Icon.svelte";
  import type { IconName } from "$lib/components/ui/icons";
  import StatusPill from "$lib/components/ui/StatusPill.svelte";
  import { providerStore } from "$lib/stores/providers.svelte";

  type GrokStatus = {
    installed: boolean;
    version: string | null;
    path: string | null;
    authenticated: boolean;
  };

  type CommandResult = {
    success: boolean;
    message: string;
  };

  let status = $state<GrokStatus | null>(null);
  let loading = $state(true);
  let busy = $state<string | null>(null);
  let notice = $state<string | null>(null);
  let signingIn = $state(false);

  let pollTimer: ReturnType<typeof setInterval> | null = null;
  let pollTimeout: ReturnType<typeof setTimeout> | null = null;

  function stopAuthPoll() {
    if (pollTimer) clearInterval(pollTimer);
    if (pollTimeout) clearTimeout(pollTimeout);
    pollTimer = null;
    pollTimeout = null;
    signingIn = false;
  }

  async function refresh(silent = false) {
    if (!silent) loading = true;
    try {
      status = await invoke<GrokStatus>("get_grok_status");
    } catch {
      status = { installed: false, version: null, path: null, authenticated: false };
    } finally {
      if (!silent) loading = false;
    }
  }

  onMount(() => {
    refresh();
    return stopAuthPoll;
  });

  async function openLogin() {
    busy = "login";
    notice = null;
    stopAuthPoll();

    try {
      const result = await invoke<CommandResult>("open_grok_login");
      if (!result.success) {
        notice = result.message;
        return;
      }

      signingIn = true;
      notice = result.message;

      pollTimer = setInterval(async () => {
        await refresh(true);
        if (status?.authenticated) {
          stopAuthPoll();
          notice = "Signed in successfully.";
        }
      }, 1500);

      pollTimeout = setTimeout(() => {
        if (signingIn) {
          stopAuthPoll();
          notice = "Sign-in timed out. Try again if your browser didn't open.";
        }
      }, 120_000);
    } catch (error) {
      notice = String(error);
      stopAuthPoll();
    } finally {
      busy = null;
    }
  }

  async function runAction(action: string, command: () => Promise<CommandResult>) {
    busy = action;
    notice = null;
    try {
      const result = await command();
      notice = result.message;
      if (result.success) await refresh(true);
    } catch (error) {
      notice = String(error);
    } finally {
      busy = null;
    }
  }

  const quickLinks: { href: string; icon: IconName; label: string; desc: string }[] = [
    { href: "/projects", icon: "folder", label: "Projects", desc: "Repos & chats" },
    { href: "/automations", icon: "zap", label: "Automations", desc: "Triggered agents" },
    { href: "/memories", icon: "memory", label: "Memories", desc: "Grok memory" },
    { href: "/skills", icon: "skills", label: "Skills", desc: "Installed skills" },
    { href: "/settings", icon: "settings", label: "Settings", desc: "Providers & theme" },
  ];
</script>

<div class="page">
  <header class="hero">
    <Wordmark size="lg" />
    <p class="tagline">
      Install Grok Build, sign in, add a project folder, and start chatting. Switch agents anytime in
      Settings.
    </p>
  </header>

  <section class="card status-card">
    <div class="card-head">
      <h2 class="card-title">Grok Build CLI</h2>
      {#if !loading && status?.version}
        <span class="mono-label">{status.version}</span>
      {/if}
    </div>

    {#if loading}
      <p class="muted">Checking installation…</p>
    {:else if status?.installed}
      <div class="pills">
        {#if status.path}
          <span class="path-tip" aria-label="Grok install path: {status.path}">
            <StatusPill tone="success" label="Installed" />
            <span class="path-popup" role="tooltip">{status.path}</span>
          </span>
        {:else}
          <StatusPill tone="success" label="Installed" />
        {/if}
        {#if status.authenticated}
          <StatusPill tone="success" label="Signed in" />
        {:else}
          <StatusPill tone="warning" label="Not signed in" />
        {/if}
      </div>
      <div class="actions">
        {#if !status.authenticated || signingIn}
          <button
            class="btn btn-primary"
            type="button"
            disabled={busy !== null || signingIn}
            onclick={openLogin}
          >
            <Icon name="user" size={15} />
            {signingIn ? "Waiting for browser…" : busy === "login" ? "Starting…" : "Sign In"}
          </button>
        {/if}
        <button
          class="btn"
          type="button"
          disabled={busy !== null}
          onclick={() => runAction("update", () => invoke<CommandResult>("check_grok_updates"))}
        >
          <Icon name="refresh" size={15} />
          {busy === "update" ? "Checking…" : "Check for Updates"}
        </button>
      </div>
    {:else}
      <div class="pills">
        <StatusPill tone="warning" label="Not installed" />
      </div>
      <p class="muted">Install Grok Build CLI to get started.</p>
      <div class="actions">
        <button
          class="btn btn-primary"
          type="button"
          disabled={busy !== null}
          onclick={() => runAction("install", () => invoke<CommandResult>("install_grok"))}
        >
          <Icon name="plus" size={15} />
          {busy === "install" ? "Installing…" : "Install Grok Build"}
        </button>
      </div>
    {/if}

    {#if notice}
      <p class="notice" class:ok={notice.includes("successfully")}>{notice}</p>
    {/if}
  </section>

  <section class="card provider-card">
    <div class="card-head">
      <h2 class="card-title">Active provider</h2>
      <a class="link" href="/settings">Manage</a>
    </div>
    <div class="provider-row">
      <span class="swatch" style="--c: {providerStore.active.accent}"></span>
      <div class="provider-meta">
        <span class="provider-name">{providerStore.active.label}</span>
        <span class="provider-kind mono-label">{providerStore.active.kind}</span>
      </div>
      {#if providerStore.active.available}
        <StatusPill tone="accent" label="Ready" />
      {:else}
        <StatusPill tone="muted" label="Unavailable" />
      {/if}
    </div>
  </section>

  <section class="quick">
    <h2 class="section-title mono-label">Quick links</h2>
    <div class="grid">
      {#each quickLinks as link}
        <a class="tile" href={link.href}>
          <span class="tile-icon"><Icon name={link.icon} size={18} /></span>
          <span class="tile-text">
            <span class="tile-label">{link.label}</span>
            <span class="tile-desc">{link.desc}</span>
          </span>
        </a>
      {/each}
    </div>
  </section>
</div>

<style>
  .page {
    max-width: var(--reading-width);
    margin-inline: auto;
  }
  .hero {
    padding: 1rem 0 1.5rem;
  }
  .tagline {
    margin-top: 0.6rem;
    color: var(--text-secondary);
    font-size: 0.9375rem;
    max-width: 52ch;
  }

  .card-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem;
    margin-bottom: 0.9rem;
  }
  .card-title {
    font-size: 0.9375rem;
    font-weight: 600;
  }
  .link {
    font-size: 0.8125rem;
    color: var(--accent);
  }
  .link:hover {
    text-decoration: underline;
  }

  .pills {
    display: flex;
    gap: 0.5rem;
    flex-wrap: wrap;
    margin-bottom: 0.6rem;
  }
  .muted {
    color: var(--text-secondary);
    font-size: 0.875rem;
  }
  .path-tip {
    position: relative;
    display: inline-flex;
    cursor: help;
  }
  .path-popup {
    position: absolute;
    left: 0;
    top: calc(100% + 0.4rem);
    z-index: 20;
    min-width: 12rem;
    max-width: min(28rem, 70vw);
    padding: 0.5rem 0.65rem;
    border-radius: var(--radius-sm);
    border: 1px solid var(--border);
    background: var(--bg-surface);
    box-shadow: var(--shadow-sm);
    font-family: var(--font-mono);
    font-size: 0.6875rem;
    line-height: 1.45;
    color: var(--text-secondary);
    word-break: break-all;
    pointer-events: none;
    opacity: 0;
    transform: translateY(-2px);
    transition:
      opacity var(--dur-fast) var(--ease),
      transform var(--dur-fast) var(--ease);
  }
  .path-tip:hover .path-popup,
  .path-tip:focus-within .path-popup {
    opacity: 1;
    transform: translateY(0);
  }
  .actions {
    display: flex;
    gap: 0.5rem;
    margin-top: 1rem;
    flex-wrap: wrap;
  }
  .notice {
    margin-top: 1rem;
    padding: 0.7rem 0.8rem;
    border-radius: var(--radius-sm);
    background: var(--bg-muted);
    border: 1px solid var(--border);
    font-size: 0.8125rem;
    color: var(--text-secondary);
    white-space: pre-wrap;
  }
  .notice.ok {
    background: var(--success-tint);
    border-color: transparent;
    color: var(--success);
  }

  .provider-row {
    display: flex;
    align-items: center;
    gap: 0.75rem;
  }
  .swatch {
    width: 12px;
    height: 12px;
    border-radius: 4px;
    background: var(--c, var(--sc-accent));
    box-shadow: var(--glow) var(--c, var(--sc-accent));
    flex: none;
  }
  .provider-meta {
    display: flex;
    flex-direction: column;
    gap: 0.1rem;
    flex: 1;
  }
  .provider-name {
    font-weight: 600;
    font-size: 0.875rem;
  }

  .quick {
    margin-top: 1.5rem;
  }
  .section-title {
    margin-bottom: 0.7rem;
  }
  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
    gap: 0.6rem;
  }
  .tile {
    display: flex;
    align-items: center;
    gap: 0.7rem;
    padding: 0.85rem;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--bg-surface);
    transition:
      border-color var(--dur-fast) var(--ease),
      background var(--dur-fast) var(--ease),
      transform var(--dur-fast) var(--ease);
  }
  .tile:hover {
    border-color: var(--sc-accent);
    transform: translateY(-1px);
  }
  .tile-icon {
    display: grid;
    place-items: center;
    width: 2.1rem;
    height: 2.1rem;
    border-radius: var(--radius-sm);
    background: var(--sc-accent-tint);
    color: var(--sc-accent);
    flex: none;
  }
  .tile-text {
    display: flex;
    flex-direction: column;
    min-width: 0;
  }
  .tile-label {
    font-weight: 600;
    font-size: 0.875rem;
  }
  .tile-desc {
    font-size: 0.75rem;
    color: var(--text-muted);
  }
</style>

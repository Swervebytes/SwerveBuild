<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { theme, type ThemePref } from "$lib/stores/theme.svelte";
  import Icon from "$lib/components/ui/Icon.svelte";
  import type { IconName } from "$lib/components/ui/icons";
  import Field from "$lib/components/ui/Field.svelte";
  import ProviderList from "$lib/components/providers/ProviderList.svelte";
  import StatusPill from "$lib/components/ui/StatusPill.svelte";

  type GrokStatus = {
    installed: boolean;
    version: string | null;
    path: string | null;
    authenticated: boolean;
  };

  let grokStatus = $state<GrokStatus | null>(null);

  const themeOptions: { value: ThemePref; label: string; icon: IconName }[] = [
    { value: "system", label: "System", icon: "settings" },
    { value: "light", label: "Light", icon: "sun" },
    { value: "dark", label: "Dark", icon: "moon" },
  ];

  onMount(async () => {
    try {
      grokStatus = await invoke<GrokStatus>("get_grok_status");
    } catch {
      grokStatus = { installed: false, version: null, path: null, authenticated: false };
    }
  });
</script>

<div class="page">
  <header class="page-header">
    <h1 class="page-title">Settings</h1>
    <p class="page-subtitle">Appearance, providers, and Grok Build</p>
  </header>

  <section class="card">
    <h2 class="group-title">Appearance</h2>
    <Field label="Theme" hint="System follows your OS light/dark preference.">
      <div class="segmented" role="radiogroup" aria-label="Theme">
        {#each themeOptions as opt}
          <button
            class="seg"
            class:active={theme.pref === opt.value}
            type="button"
            role="radio"
            aria-checked={theme.pref === opt.value}
            onclick={() => theme.set(opt.value)}
          >
            <Icon name={opt.icon} size={14} />
            {opt.label}
          </button>
        {/each}
      </div>
    </Field>
  </section>

  <section class="card">
    <div class="group-head">
      <h2 class="group-title">Providers</h2>
      <span class="mono-label">Grok default · any ACP agent</span>
    </div>
    <p class="group-note">
      Choose which agent backs your chats. Grok Build works out of the box after install and sign-in
      on the home screen. Other ACP agents (Claude Code, Gemini) become available when their CLI is
      on your PATH. HTTP / local-LLM providers are designed and coming soon.
    </p>
    <ProviderList />
  </section>

  <section class="card">
    <div class="group-head">
      <h2 class="group-title">Grok Build</h2>
      <a class="link" href="/">Install / sign in</a>
    </div>
    {#if grokStatus?.installed}
      <div class="grok-row">
        <StatusPill tone="success" label="Installed" />
        {#if grokStatus.authenticated}
          <StatusPill tone="success" label="Signed in" />
        {:else}
          <StatusPill tone="warning" label="Not signed in" />
        {/if}
      </div>
      {#if grokStatus.path}
        <p class="grok-path mono-label">{grokStatus.path}</p>
      {/if}
      {#if grokStatus.version}
        <p class="grok-version muted">{grokStatus.version}</p>
      {/if}
    {:else}
      <p class="group-note">Grok Build is not installed. Use the home screen to install and sign in.</p>
    {/if}
  </section>

  <section class="card">
    <h2 class="group-title">Application</h2>
    <p class="group-note">
      Up to 3 chats stay connected in the background. Switching between them reconnects instantly;
      older sessions pause automatically when a fourth chat opens.
    </p>
  </section>

  <section class="card about">
    <h2 class="group-title">About</h2>
    <p class="about-line">Swerve Build v0.1.1</p>
    <p class="about-line muted">MIT License · Open source</p>
  </section>
</div>

<style>
  .page {
    max-width: 640px;
    margin-inline: auto;
  }
  .page-header {
    margin-bottom: 1.25rem;
  }
  .group-title {
    font-size: 0.9375rem;
    font-weight: 600;
    margin-bottom: 0.9rem;
  }
  .group-head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 0.75rem;
    margin-bottom: 0.5rem;
  }
  .group-head .group-title {
    margin-bottom: 0;
  }
  .group-note {
    font-size: 0.8125rem;
    color: var(--text-muted);
    line-height: 1.5;
    margin-bottom: 1rem;
  }
  .link {
    font-size: 0.8125rem;
    color: var(--accent);
  }
  .link:hover {
    text-decoration: underline;
  }
  .grok-row {
    display: flex;
    gap: 0.5rem;
    flex-wrap: wrap;
    margin-bottom: 0.5rem;
  }
  .grok-path {
    font-size: 0.75rem;
    color: var(--text-secondary);
    word-break: break-all;
    margin-bottom: 0.25rem;
  }
  .grok-version {
    font-size: 0.8125rem;
  }
  .muted {
    color: var(--text-muted);
  }

  .segmented {
    display: inline-flex;
    padding: 3px;
    gap: 2px;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--bg-muted);
  }
  .seg {
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
    padding: 0.4rem 0.7rem;
    border: 0;
    border-radius: var(--radius-sm);
    background: transparent;
    color: var(--text-secondary);
    font-size: 0.8125rem;
    font-weight: 500;
    cursor: pointer;
    transition:
      background var(--dur-fast) var(--ease),
      color var(--dur-fast) var(--ease);
  }
  .seg:hover {
    color: var(--text-primary);
  }
  .seg.active {
    background: var(--sc-accent);
    color: #04060d;
  }

  .about-line {
    font-size: 0.875rem;
    margin-bottom: 0.25rem;
  }
</style>
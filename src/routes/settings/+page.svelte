<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import { theme, type ThemePref } from "$lib/stores/theme.svelte";
  import Icon from "$lib/components/ui/Icon.svelte";
  import type { IconName } from "$lib/components/ui/icons";
  import Field from "$lib/components/ui/Field.svelte";
  import ProviderList from "$lib/components/providers/ProviderList.svelte";
  import StatusPill from "$lib/components/ui/StatusPill.svelte";
  import LocalModelsCard from "$lib/components/models/LocalModelsCard.svelte";
  import { APP_VERSION } from "$lib/version";

  type GrokStatus = {
    installed: boolean;
    version: string | null;
    path: string | null;
    authenticated: boolean;
  };

  type GrokEndpoint = {
    enabled: boolean;
    base_url: string;
    model: string;
    api_backend: string;
    context_window: number | null;
    has_api_key: boolean;
    config_path: string;
  };

  type ModelInfo = {
    id: string;
    label: string;
    kind: "hosted" | "custom" | "endpoint";
    note: string | null;
    is_default: boolean;
  };

  let grokStatus = $state<GrokStatus | null>(null);

  let customIds = $state("");
  let idsSaving = $state(false);
  let idsMessage = $state<string | null>(null);

  let endpoint = $state<GrokEndpoint | null>(null);
  let apiKeyInput = $state("");
  let epSaving = $state(false);
  let epTesting = $state(false);
  let epMessage = $state<string | null>(null);
  let epError = $state(false);

  let appUiGranted = $state(false);
  let appUiSaving = $state(false);
  let appUiMessage = $state<string | null>(null);
  let termGranted = $state(false);
  let termSaving = $state(false);
  let termMessage = $state<string | null>(null);

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
    try {
      endpoint = await invoke<GrokEndpoint>("get_grok_endpoint");
    } catch (e) {
      epMessage = String(e);
      epError = true;
    }
    try {
      const models = await invoke<ModelInfo[]>("list_models");
      customIds = models
        .filter((m) => m.kind === "custom")
        .map((m) => m.id)
        .join(", ");
    } catch {
      /* model list is optional here */
    }
    try {
      const grant = await invoke<{ granted: boolean }>("get_app_ui_grant");
      appUiGranted = !!grant.granted;
    } catch {
      appUiGranted = false;
    }
    try {
      const grant = await invoke<{ granted: boolean }>("get_term_grant");
      termGranted = !!grant.granted;
    } catch {
      termGranted = false;
    }
  });

  async function setAppUiGrant(next: boolean) {
    appUiSaving = true;
    appUiMessage = null;
    try {
      const grant = await invoke<{ granted: boolean }>("set_app_ui_grant", {
        granted: next,
      });
      appUiGranted = !!grant.granted;
      appUiMessage = appUiGranted
        ? "Agents in this app may use app_ui_* tools (read-only route/title today)."
        : "App UI control revoked.";
    } catch (e) {
      appUiMessage = String(e);
    } finally {
      appUiSaving = false;
    }
  }

  async function setTermGrant(next: boolean) {
    termSaving = true;
    termMessage = null;
    try {
      const grant = await invoke<{ granted: boolean }>("set_term_grant", {
        granted: next,
      });
      termGranted = !!grant.granted;
      termMessage = termGranted
        ? "Agents in this app may run one-shot terminal commands in the open project."
        : "Agent terminal access revoked.";
    } catch (e) {
      termMessage = String(e);
    } finally {
      termSaving = false;
    }
  }

  async function saveCustomIds() {
    idsSaving = true;
    idsMessage = null;
    try {
      const ids = customIds
        .split(",")
        .map((s) => s.trim())
        .filter(Boolean);
      const models = await invoke<ModelInfo[]>("set_custom_model_ids", { ids });
      customIds = models
        .filter((m) => m.kind === "custom")
        .map((m) => m.id)
        .join(", ");
      idsMessage = "Saved — these IDs now appear in the model pickers.";
    } catch (e) {
      idsMessage = String(e);
    } finally {
      idsSaving = false;
    }
  }

  async function saveEndpoint() {
    if (!endpoint) return;
    epSaving = true;
    epMessage = null;
    epError = false;
    try {
      const input = {
        enabled: endpoint.enabled,
        base_url: endpoint.base_url,
        model: endpoint.model,
        api_backend: endpoint.api_backend,
        context_window: endpoint.context_window,
        // Only send a key when the user typed one; blank keeps the stored key.
        api_key: apiKeyInput.trim() === "" ? null : apiKeyInput,
      };
      endpoint = await invoke<GrokEndpoint>("set_grok_endpoint", { input });
      apiKeyInput = "";
      epMessage = endpoint.enabled
        ? "Saved — Grok now routes through your endpoint."
        : "Saved — Grok uses its default hosted routing.";
    } catch (e) {
      epMessage = String(e);
      epError = true;
    } finally {
      epSaving = false;
    }
  }

  async function testEndpoint() {
    epTesting = true;
    epMessage = null;
    epError = false;
    try {
      const r = await invoke<{ success: boolean; message: string }>("test_grok_endpoint");
      epMessage = r.message;
      epError = !r.success;
    } catch (e) {
      epMessage = String(e);
      epError = true;
    } finally {
      epTesting = false;
    }
  }
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
    <div class="ids-block">
      <Field
        label="Extra model IDs"
        hint="Comma-separated hosted model IDs to add to the model pickers (e.g. a beta ID). Chats and automations pass the ID to Grok via -m."
      >
        <div class="ids-row">
          <input
            class="input"
            type="text"
            spellcheck="false"
            autocapitalize="off"
            placeholder="grok-4.5-mini, my-beta-model"
            bind:value={customIds}
          />
          <button class="btn btn-sm" type="button" disabled={idsSaving} onclick={saveCustomIds}>
            {idsSaving ? "Saving…" : "Save"}
          </button>
        </div>
      </Field>
      {#if idsMessage}
        <p class="ep-msg">{idsMessage}</p>
      {/if}
    </div>
  </section>

  <section class="card">
    <div class="group-head">
      <h2 class="group-title">Local models</h2>
      <span class="mono-label">llama.cpp · runs on this machine</span>
    </div>
    <p class="group-note">
      Run models entirely on this PC — no Ollama, no cloud, no sign-in. Swerve downloads its own
      inference engine once, then any <code>.gguf</code> you add becomes a model selectable in chats
      and automations. One local model is loaded at a time; the server listens on localhost only.
    </p>
    <LocalModelsCard />
  </section>

  <section class="card">
    <div class="group-head">
      <h2 class="group-title">Custom endpoint (advanced)</h2>
      <StatusPill
        tone={endpoint?.enabled ? "success" : "muted"}
        label={endpoint?.enabled ? "Routing on" : "Off"}
      />
    </div>
    <p class="group-note">
      Point Grok Build at your own OpenAI-compatible inference — local, self-hosted, or a gateway — so
      your code never leaves the machine. Swerve writes a managed <code>[model.swerve-endpoint]</code>
      block to your <code>~/.grok/config.toml</code> (backed up first) and, while routing is on, sets it
      as Grok's default model. The API key is injected at launch, never written to the config file.
    </p>
    {#if endpoint}
      <Field label="Base URL" hint="Your endpoint's OpenAI-compatible base, e.g. http://localhost:11434/v1">
        <input
          class="input"
          type="text"
          spellcheck="false"
          autocapitalize="off"
          placeholder="http://localhost:11434/v1"
          bind:value={endpoint.base_url}
        />
      </Field>
      <Field label="Model" hint="Model id the endpoint serves.">
        <input
          class="input"
          type="text"
          spellcheck="false"
          autocapitalize="off"
          placeholder="qwen2.5-coder:14b"
          bind:value={endpoint.model}
        />
      </Field>
      <Field
        label="API key"
        hint={endpoint.has_api_key
          ? "A key is saved — leave blank to keep it, or type a new one to replace."
          : "Passed to your endpoint via env; stored locally, never sent to xAI."}
      >
        <input
          class="input"
          type="password"
          autocomplete="off"
          placeholder={endpoint.has_api_key ? "•••••••• (saved)" : "sk-…"}
          bind:value={apiKeyInput}
        />
      </Field>
      <Field label="API backend" hint="Wire format your endpoint speaks.">
        <select class="input" bind:value={endpoint.api_backend}>
          <option value="">chat_completions (default)</option>
          <option value="responses">responses</option>
          <option value="anthropic">anthropic</option>
        </select>
      </Field>
      <Field label="Route Grok through this endpoint" hint="Off keeps xAI's hosted models." row>
        <div class="segmented" role="radiogroup" aria-label="Endpoint routing">
          <button
            class="seg"
            class:active={!endpoint.enabled}
            type="button"
            role="radio"
            aria-checked={!endpoint.enabled}
            onclick={() => endpoint && (endpoint.enabled = false)}
          >
            Off
          </button>
          <button
            class="seg"
            class:active={endpoint.enabled}
            type="button"
            role="radio"
            aria-checked={endpoint.enabled}
            onclick={() => endpoint && (endpoint.enabled = true)}
          >
            On
          </button>
        </div>
      </Field>
      <div class="ep-actions">
        <button class="btn btn-sm" type="button" disabled={epSaving} onclick={saveEndpoint}>
          {epSaving ? "Saving…" : "Save endpoint"}
        </button>
        <button class="btn btn-sm btn-ghost" type="button" disabled={epTesting} onclick={testEndpoint}>
          <Icon name="refresh" size={13} />
          {epTesting ? "Checking…" : "Test"}
        </button>
      </div>
      {#if epMessage}
        <p class="ep-msg" class:error={epError}>{epMessage}</p>
      {/if}
      <p class="ep-path mono-label">{endpoint.config_path}</p>
    {:else}
      <p class="group-note">Loading endpoint settings…</p>
    {/if}
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
    <div class="group-head">
      <h2 class="group-title">Agent UI control</h2>
      <StatusPill
        tone={appUiGranted ? "warning" : "muted"}
        label={appUiGranted ? "Granted" : "Off"}
      />
    </div>
    <p class="group-note">
      Allow the in-app agent to call <code>app_ui_*</code> MCP tools against <strong>this</strong>
      SwerveBuild window (route/title first; click/type/screenshot after CDP lands). Off by default.
      Never enabled for automations. Revoke anytime.
    </p>
    <Field
      label="Allow agent to control SwerveBuild UI"
      hint="First step of the self-dev loop. Does not bypass file/shell permission prompts."
      row
    >
      <div class="segmented" role="radiogroup" aria-label="App UI grant">
        <button
          class="seg"
          class:active={!appUiGranted}
          type="button"
          role="radio"
          aria-checked={!appUiGranted}
          disabled={appUiSaving}
          onclick={() => setAppUiGrant(false)}
        >
          Off
        </button>
        <button
          class="seg"
          class:active={appUiGranted}
          type="button"
          role="radio"
          aria-checked={appUiGranted}
          disabled={appUiSaving}
          data-testid="app-ui-grant-on"
          onclick={() => setAppUiGrant(true)}
        >
          On
        </button>
      </div>
    </Field>
    {#if appUiMessage}
      <p class="ep-msg">{appUiMessage}</p>
    {/if}
  </section>

  <section class="card">
    <div class="group-head">
      <h2 class="group-title">Agent terminal</h2>
      <StatusPill
        tone={termGranted ? "warning" : "muted"}
        label={termGranted ? "Granted" : "Off"}
      />
    </div>
    <p class="group-note">
      Allow the in-app agent to run shell commands — <strong>one-shot</strong> (<code>term_run</code>)
      and <strong>persistent sessions</strong> (<code>term_start</code>/<code>term_exec</code>, which
      keep a live PowerShell where state persists across commands). Sessions start inside the open
      project and die with the app. Off by default. Never enabled for automations. Revoke anytime.
    </p>
    <Field
      label="Allow agent to run terminal commands"
      hint="PowerShell. One-shot runs are cwd-confined; a persistent session can cd anywhere you can — the grant is the gate."
      row
    >
      <div class="segmented" role="radiogroup" aria-label="Agent terminal grant">
        <button
          class="seg"
          class:active={!termGranted}
          type="button"
          role="radio"
          aria-checked={!termGranted}
          disabled={termSaving}
          onclick={() => setTermGrant(false)}
        >
          Off
        </button>
        <button
          class="seg"
          class:active={termGranted}
          type="button"
          role="radio"
          aria-checked={termGranted}
          disabled={termSaving}
          data-testid="term-grant-on"
          onclick={() => setTermGrant(true)}
        >
          On
        </button>
      </div>
    </Field>
    {#if termMessage}
      <p class="ep-msg">{termMessage}</p>
    {/if}
  </section>

  <section class="card">
    <div class="group-head">
      <h2 class="group-title">Automations &amp; safety</h2>
      <StatusPill tone="accent" label="Shadow mode default" />
    </div>
    <p class="group-note">
      Automations run Grok headless on a trigger. By default they use read-only tools — enforced in
      the app itself, not just the UI — so they can read and reason about your project but cannot
      change files. Grok's own approval prompts and the OS sandbox don't apply to background runs on
      Windows, so anything that could write is deliberately gated and confined to the automation's
      project folder. Up to 2 automations run at once, and they run while Swerve Build is open.
    </p>
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
    <p class="about-line">Swerve Build v{APP_VERSION}</p>
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

  .input {
    width: 100%;
    padding: 0.5rem 0.6rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    background: var(--bg-muted);
    color: var(--text-primary);
    font-family: inherit;
    font-size: 0.8125rem;
  }
  .input:focus {
    outline: none;
    border-color: var(--sc-accent);
  }
  select.input {
    cursor: pointer;
  }
  .ep-actions {
    display: flex;
    gap: 0.5rem;
    margin-top: 1rem;
  }
  .ids-block {
    margin-top: 1rem;
  }
  .ids-row {
    display: flex;
    gap: 0.5rem;
    align-items: center;
  }
  .ids-row .input {
    flex: 1;
  }
  .ep-msg {
    font-size: 0.8125rem;
    color: var(--text-secondary);
    margin-top: 0.6rem;
    word-break: break-word;
  }
  .ep-msg.error {
    color: var(--danger, #ff6b6b);
  }
  .ep-path {
    font-size: 0.7rem;
    color: var(--text-faint);
    margin-top: 0.5rem;
    word-break: break-all;
  }
  code {
    font-family: var(--font-mono);
    font-size: 0.85em;
  }
</style>
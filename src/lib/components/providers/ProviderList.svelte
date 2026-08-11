<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { providerStore } from "$lib/stores/providers.svelte";
  import Icon from "$lib/components/ui/Icon.svelte";
  import StatusPill from "$lib/components/ui/StatusPill.svelte";

  const providers = $derived(providerStore.all);

  type InstallInfo = {
    installable: boolean;
    package: string | null;
    version: string | null;
    docs: string | null;
    installCommand: string | null;
    uninstallCommand: string | null;
    npmVersion: string | null;
    installed: boolean;
  };

  let testing = $state<string | null>(null);
  let results = $state<Record<string, string>>({});
  let info = $state<Record<string, InstallInfo>>({});
  let busy = $state<string | null>(null);
  /** Provider id awaiting uninstall confirmation. */
  let confirming = $state<string | null>(null);

  // Load install capability per provider so buttons reflect reality (is it
  // managed by us? is npm even here?) rather than assuming.
  $effect(() => {
    for (const p of providers) {
      if (info[p.id]) continue;
      invoke<InstallInfo>("provider_install_info", { id: p.id })
        .then((r) => (info = { ...info, [p.id]: r }))
        .catch(() => {});
    }
  });

  async function refreshInfo(id: string) {
    try {
      info = { ...info, [id]: await invoke<InstallInfo>("provider_install_info", { id }) };
    } catch {
      /* leave previous info */
    }
  }

  async function install(id: string) {
    busy = id;
    try {
      const r = await invoke<{ success: boolean; message: string }>("install_provider_cli", { id });
      results = { ...results, [id]: r.message };
      await providerStore.load();
      await refreshInfo(id);
    } catch (e) {
      results = { ...results, [id]: String(e) };
    } finally {
      busy = null;
    }
  }

  async function uninstall(id: string) {
    confirming = null;
    busy = id;
    try {
      const r = await invoke<{ success: boolean; message: string }>("uninstall_provider_cli", { id });
      results = { ...results, [id]: r.message };
      await providerStore.load();
      await refreshInfo(id);
    } catch (e) {
      results = { ...results, [id]: String(e) };
    } finally {
      busy = null;
    }
  }

  async function activate(id: string, available: boolean) {
    if (!available) return;
    await providerStore.setActive(id);
  }

  async function test(id: string) {
    testing = id;
    try {
      const r = await invoke<{ success: boolean; message: string }>("test_provider", { id });
      results = { ...results, [id]: r.message };
    } catch (e) {
      results = { ...results, [id]: String(e) };
    } finally {
      testing = null;
    }
  }

  // ---- sign-in (P1.1 / S-AUTH) ------------------------------------------
  // Installing an agent does not sign it in. Probe asks the agent itself for
  // its authMethods (ACP initialize); one method runs immediately, several
  // render an inline chooser. The flows are the agent's own (browser OAuth or
  // a visible terminal) — we never collect credentials in our UI.

  type AuthMethod = { id: string; name: string; description: string };

  let signingIn = $state<string | null>(null);
  /** Provider id → methods, only while the inline chooser is open. */
  let methodChoices = $state<Record<string, AuthMethod[]>>({});

  async function signIn(id: string) {
    signingIn = id;
    methodChoices = { ...methodChoices, [id]: [] };
    try {
      const probe = await invoke<{ authMethods: AuthMethod[] }>("provider_auth_probe", {
        providerId: id,
      });
      const methods = probe.authMethods;
      if (methods.length === 0) {
        results = {
          ...results,
          [id]: "This agent reports no sign-in method — it may already be signed in. Send a chat message to check.",
        };
      } else if (methods.length === 1) {
        await runSignIn(id, methods[0].id);
      } else {
        methodChoices = { ...methodChoices, [id]: methods };
      }
    } catch (e) {
      results = { ...results, [id]: String(e) };
    } finally {
      signingIn = null;
    }
  }

  async function runSignIn(id: string, methodId: string) {
    signingIn = id;
    methodChoices = { ...methodChoices, [id]: [] };
    try {
      const r = await invoke<{ kind: string; message: string }>("provider_sign_in", {
        providerId: id,
        methodId,
      });
      results = { ...results, [id]: r.message };
    } catch (e) {
      results = { ...results, [id]: String(e) };
    } finally {
      signingIn = null;
    }
  }

  /// HTTP rows that shipped features already cover — say so instead of
  /// promising "soon" forever (S37).
  const COVERED_BY: Record<string, string> = {
    ollama: "Use Local models",
    "openai-compatible": "Use Grok custom endpoint",
  };

  function availability(p: { id: string; available: boolean; kind: string }) {
    if (p.available) return { tone: "success" as const, label: "Available" };
    if (COVERED_BY[p.id]) return { tone: "muted" as const, label: COVERED_BY[p.id] };
    if (p.kind === "http") return { tone: "muted" as const, label: "Not built yet" };
    return { tone: "muted" as const, label: "Not installed" };
  }
</script>

<div class="list">
  {#each providers as p (p.id)}
    {@const avail = availability(p)}
    <div class="row" class:active={p.active}>
      <span class="swatch" style="--c: {p.accent}"></span>
      <div class="meta">
        <div class="top">
          <span class="name">{p.label}</span>
          <span class="kind">{p.kind === "acp" ? "ACP" : "HTTP"}</span>
          {#if p.active}<span class="badge badge-accent">Active</span>{/if}
        </div>
        <span class="id">{p.id}{#if p.command} · {p.command}{/if}</span>
        {#if COVERED_BY[p.id]}
          <span class="note">
            {p.id === "ollama"
              ? "Local GGUF models already run in-app via the managed llama-server — see Local models below."
              : "Any OpenAI-compatible endpoint already works via the Grok custom endpoint below."}
          </span>
        {/if}
        {#if info[p.id]?.installable && !info[p.id]?.npmVersion}
          <span class="note">
            npm not found — install Node.js, then run
            <code>{info[p.id]?.installCommand}</code>
          </span>
        {/if}
        {#if (methodChoices[p.id] ?? []).length > 1}
          <span class="confirm" data-testid="provider-method-choice">
            Sign in with:
            {#each methodChoices[p.id] as m (m.id)}
              <button
                class="btn btn-sm"
                type="button"
                data-testid="provider-method-{m.id}"
                title={m.description}
                onclick={() => runSignIn(p.id, m.id)}
              >
                {m.name}
              </button>
            {/each}
            <button
              class="btn btn-sm btn-ghost"
              type="button"
              onclick={() => (methodChoices = { ...methodChoices, [p.id]: [] })}
            >
              Cancel
            </button>
          </span>
        {/if}
        {#if confirming === p.id}
          <span class="confirm" data-testid="provider-uninstall-confirm">
            Remove this CLI from your machine? Runs
            <code>{info[p.id]?.uninstallCommand}</code>
            <button class="btn btn-sm" type="button" onclick={() => uninstall(p.id)}>
              Yes, uninstall
            </button>
            <button class="btn btn-sm btn-ghost" type="button" onclick={() => (confirming = null)}>
              Cancel
            </button>
          </span>
        {/if}
        {#if results[p.id]}<span class="result">{results[p.id]}</span>{/if}
      </div>
      <div class="side">
        <StatusPill tone={avail.tone} label={avail.label} />
        <div class="btns">
          {#if !p.active}
            <button
              class="btn btn-sm"
              type="button"
              data-testid="provider-activate-{p.id}"
              disabled={!p.available}
              onclick={() => activate(p.id, p.available)}
            >
              Set active
            </button>
          {/if}
          {#if info[p.id]?.installable && !p.available}
            <button
              class="btn btn-sm"
              type="button"
              data-testid="provider-install-{p.id}"
              disabled={busy === p.id || !info[p.id]?.npmVersion}
              title={info[p.id]?.installCommand ?? ""}
              onclick={() => install(p.id)}
            >
              {busy === p.id ? "Installing…" : "Install"}
            </button>
          {/if}
          {#if p.kind === "acp" && p.command && p.available}
            <button
              class="btn btn-sm"
              type="button"
              data-testid="provider-signin-{p.id}"
              disabled={signingIn === p.id}
              title="Run this agent's own sign-in flow (browser or terminal)"
              onclick={() => signIn(p.id)}
            >
              {signingIn === p.id ? "Signing in…" : "Sign in"}
            </button>
          {/if}
          {#if info[p.id]?.installable && p.available}
            <button
              class="btn btn-sm btn-ghost"
              type="button"
              data-testid="provider-uninstall-{p.id}"
              disabled={busy === p.id}
              title={info[p.id]?.uninstallCommand ?? ""}
              onclick={() => (confirming = confirming === p.id ? null : p.id)}
            >
              {busy === p.id ? "Removing…" : "Uninstall"}
            </button>
          {/if}
          <button
            class="btn btn-sm btn-ghost"
            type="button"
            disabled={testing === p.id}
            onclick={() => test(p.id)}
          >
            <Icon name="refresh" size={13} />
            {testing === p.id ? "Testing…" : "Test"}
          </button>
        </div>
      </div>
    </div>
  {/each}
</div>

<style>
  .list {
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }
  .row {
    display: flex;
    align-items: flex-start;
    gap: 0.7rem;
    padding: 0.75rem 0.85rem;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--bg-muted);
  }
  .row.active {
    border-color: color-mix(in srgb, var(--sc-accent) 45%, var(--border));
    background: var(--sc-accent-tint);
  }
  .swatch {
    width: 11px;
    height: 11px;
    border-radius: 3px;
    background: var(--c, var(--sc-accent));
    box-shadow: var(--glow) var(--c, var(--sc-accent));
    flex: none;
    margin-top: 0.3rem;
  }
  .meta {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 0.15rem;
  }
  .top {
    display: flex;
    align-items: center;
    gap: 0.5rem;
  }
  .name {
    font-weight: 600;
    font-size: 0.875rem;
  }
  .kind {
    font-family: var(--font-mono);
    font-size: 0.625rem;
    letter-spacing: 0.08em;
    color: var(--text-faint);
    border: 1px solid var(--border);
    border-radius: var(--radius-pill);
    padding: 0.05rem 0.4rem;
  }
  .id {
    font-family: var(--font-mono);
    font-size: 0.75rem;
    color: var(--text-muted);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .result {
    font-size: 0.75rem;
    color: var(--text-secondary);
    margin-top: 0.2rem;
    word-break: break-word;
  }
  .note {
    font-size: 0.75rem;
    color: var(--text-muted);
    margin-top: 0.2rem;
    word-break: break-word;
  }
  .note code,
  .confirm code {
    font-family: var(--font-mono);
    font-size: 0.7rem;
    background: var(--bg);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm, 4px);
    padding: 0.05rem 0.3rem;
  }
  .confirm {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 0.4rem;
    font-size: 0.75rem;
    color: var(--text-secondary);
    margin-top: 0.35rem;
  }
  .side {
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    gap: 0.4rem;
    flex: none;
  }
  .btns {
    display: flex;
    gap: 0.35rem;
  }
</style>

<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import { providerStore } from "$lib/stores/providers.svelte";
  import Icon from "$lib/components/ui/Icon.svelte";
  import StatusPill from "$lib/components/ui/StatusPill.svelte";

  const providers = $derived(providerStore.all);

  let testing = $state<string | null>(null);
  let results = $state<Record<string, string>>({});

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

  function availability(p: { available: boolean; kind: string }) {
    if (p.available) return { tone: "success" as const, label: "Available" };
    if (p.kind === "http") return { tone: "muted" as const, label: "Designed — soon" };
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
        {#if results[p.id]}<span class="result">{results[p.id]}</span>{/if}
      </div>
      <div class="side">
        <StatusPill tone={avail.tone} label={avail.label} />
        <div class="btns">
          {#if !p.active}
            <button
              class="btn btn-sm"
              type="button"
              disabled={!p.available}
              onclick={() => activate(p.id, p.available)}
            >
              Set active
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

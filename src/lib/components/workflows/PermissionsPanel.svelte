<script lang="ts">
  import Icon from "$lib/components/ui/Icon.svelte";
  import type { CatalogEntry, WfNode, WfPermissions } from "$lib/workflows/api";

  let {
    permissions,
    nodes,
    catalog,
    onchange,
    onclose,
  }: {
    permissions: WfPermissions;
    nodes: WfNode[];
    catalog: CatalogEntry[];
    onchange: () => void;
    onclose: () => void;
  } = $props();

  const specOf = (type: string) => catalog.find((c) => c.type === type);

  /** What the current graph actually needs, with the nodes that need it. */
  const needs = $derived.by(() => {
    const agg = { network: [] as string[], code: [] as string[], fs_read: [] as string[], fs_write: [] as string[], agent: [] as string[] };
    for (const n of nodes) {
      if (n.disabled) continue;
      const s = specOf(n.type);
      if (!s) continue;
      if (s.needs.network) agg.network.push(n.name);
      if (s.needs.code) agg.code.push(n.name);
      if (s.needs.fs_read) agg.fs_read.push(n.name);
      if (s.needs.fs_write) agg.fs_write.push(n.name);
      if (s.needs.agent) agg.agent.push(n.name);
    }
    return agg;
  });

  function lines(value: string[]): string {
    return value.join("\n");
  }

  function setLines(target: string[], raw: string) {
    target.splice(0, target.length, ...raw.split("\n").map((s) => s.trim()).filter(Boolean));
    onchange();
  }

  function hostsText(): string {
    return permissions.network.hosts.join(", ");
  }
</script>

<svelte:window onkeydown={(e) => e.key === "Escape" && onclose()} />

<div class="scrim" role="presentation" onclick={(e) => e.target === e.currentTarget && onclose()}>
  <div class="panel" role="dialog" aria-modal="true" aria-label="Workflow permissions" tabindex="-1">
    <div class="panel-head">
      <span class="panel-icon"><Icon name="shield" size={15} /></span>
      <div>
        <div class="panel-title">Permissions</div>
        <div class="panel-sub">Everything is off until you grant it. Grants apply to this workflow only.</div>
      </div>
      <button class="btn btn-ghost btn-icon" onclick={onclose}><Icon name="close" size={14} /></button>
    </div>

    <div class="perm" class:needed={needs.network.length > 0}>
      <label class="perm-main">
        <input type="checkbox" bind:checked={permissions.network.enabled} onchange={onchange} />
        <div>
          <div class="perm-name">Network</div>
          <div class="perm-desc">HTTP requests to the hosts listed below.</div>
        </div>
        {#if needs.network.length > 0}
          <span class="badge badge-warning" title={needs.network.join(", ")}>needed by {needs.network.length}</span>
        {/if}
      </label>
      {#if permissions.network.enabled}
        <div class="perm-body">
          <label class="mini-field">
            <span>Allowed hosts (comma separated, blank = any public host)</span>
            <input value={hostsText()} placeholder="api.github.com, *.roaringbytes.com" spellcheck="false"
              oninput={(e) => {
                permissions.network.hosts = e.currentTarget.value.split(",").map((s) => s.trim()).filter(Boolean);
                onchange();
              }} />
          </label>
          <label class="mini-toggle">
            <input type="checkbox" bind:checked={permissions.network.private_ips} onchange={onchange} />
            <span>Allow local and private addresses (homelab targets like 192.168.x.x)</span>
          </label>
        </div>
      {/if}
    </div>

    <div class="perm" class:needed={needs.code.length > 0}>
      <label class="perm-main">
        <input type="checkbox" bind:checked={permissions.code} onchange={onchange} />
        <div>
          <div class="perm-name">Code</div>
          <div class="perm-desc">JavaScript in Code nodes. Always sandboxed, no file or network access.</div>
        </div>
        {#if needs.code.length > 0}
          <span class="badge badge-warning" title={needs.code.join(", ")}>needed by {needs.code.length}</span>
        {/if}
      </label>
    </div>

    <div class="perm" class:needed={needs.fs_read.length + needs.fs_write.length > 0}>
      <div class="perm-main as-div">
        <span class="perm-static-icon"><Icon name="folder" size={14} /></span>
        <div>
          <div class="perm-name">Files</div>
          <div class="perm-desc">Folders this workflow may read from or write into. One per line.</div>
        </div>
        {#if needs.fs_read.length + needs.fs_write.length > 0}
          <span class="badge badge-warning">needed by {needs.fs_read.length + needs.fs_write.length}</span>
        {/if}
      </div>
      <div class="perm-body two-col">
        <label class="mini-field">
          <span>Readable folders</span>
          <textarea rows="2" value={lines(permissions.fs.read)} placeholder="C:\Path\To\Reports" spellcheck="false"
            oninput={(e) => setLines(permissions.fs.read, e.currentTarget.value)}></textarea>
        </label>
        <label class="mini-field">
          <span>Writable folders</span>
          <textarea rows="2" value={lines(permissions.fs.write)} placeholder="C:\Path\To\Reports\out" spellcheck="false"
            oninput={(e) => setLines(permissions.fs.write, e.currentTarget.value)}></textarea>
        </label>
      </div>
    </div>

    <div class="perm" class:needed={needs.agent.length > 0}>
      <label class="perm-main">
        <input type="checkbox" bind:checked={permissions.agent} onchange={onchange} />
        <div>
          <div class="perm-name">Agent</div>
          <div class="perm-desc">Read-only agent turns. Same shadow confinement as Automations, always.</div>
        </div>
        {#if needs.agent.length > 0}
          <span class="badge badge-warning" title={needs.agent.join(", ")}>needed by {needs.agent.length}</span>
        {/if}
      </label>
    </div>
  </div>
</div>

<style>
  .scrim {
    position: fixed;
    inset: 0;
    z-index: 60;
    display: grid;
    place-items: center;
    background: rgba(0, 0, 0, 0.55);
    backdrop-filter: blur(2px);
  }

  .panel {
    width: min(560px, calc(100vw - 3rem));
    max-height: calc(100vh - 6rem);
    overflow-y: auto;
    display: flex;
    flex-direction: column;
    gap: 0.75rem;
    padding: 1.1rem;
    background: var(--bg-surface);
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow-lg);
  }

  .panel-head {
    display: flex;
    align-items: flex-start;
    gap: 0.6rem;
  }

  .panel-icon {
    display: grid;
    place-items: center;
    width: 30px;
    height: 30px;
    flex: none;
    border-radius: var(--radius-sm);
    background: var(--accent-tint);
    color: var(--accent);
  }

  .panel-head > div {
    flex: 1;
  }

  .panel-title {
    font-weight: 700;
    font-size: 0.9375rem;
  }

  .panel-sub {
    font-size: 0.75rem;
    color: var(--text-muted);
  }

  .perm {
    border: 1px solid var(--border);
    border-radius: var(--radius);
    padding: 0.7rem 0.8rem;
    background: var(--bg-muted);
  }

  .perm.needed {
    border-color: color-mix(in srgb, var(--warning) 45%, var(--border));
  }

  .perm-main {
    display: flex;
    align-items: flex-start;
    gap: 0.6rem;
    cursor: pointer;
  }

  .perm-main.as-div {
    cursor: default;
  }

  .perm-main > div {
    flex: 1;
  }

  .perm-main input[type="checkbox"] {
    margin-top: 0.2rem;
  }

  .perm-static-icon {
    color: var(--text-muted);
    margin-top: 0.1rem;
  }

  .perm-name {
    font-size: 0.8125rem;
    font-weight: 600;
  }

  .perm-desc {
    font-size: 0.75rem;
    color: var(--text-muted);
  }

  .perm-body {
    margin-top: 0.6rem;
    display: flex;
    flex-direction: column;
    gap: 0.5rem;
  }

  .perm-body.two-col {
    display: grid;
    grid-template-columns: 1fr 1fr;
    gap: 0.5rem;
  }

  .mini-field {
    display: flex;
    flex-direction: column;
    gap: 0.25rem;
    font-size: 0.6875rem;
    color: var(--text-muted);
  }

  .mini-field input,
  .mini-field textarea {
    width: 100%;
    padding: 0.4rem 0.55rem;
    background: var(--bg-surface-2);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    font-size: 0.75rem;
    font-family: var(--font-mono);
    resize: vertical;
  }

  .mini-toggle {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    font-size: 0.75rem;
    color: var(--text-secondary);
    cursor: pointer;
  }
</style>

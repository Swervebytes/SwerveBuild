<script lang="ts">
  import Icon from "$lib/components/ui/Icon.svelte";
  import type { CatalogEntry } from "$lib/workflows/api";
  import { categoryColor, nodeIcon } from "$lib/workflows/model";

  let {
    catalog,
    onadd,
  }: {
    catalog: CatalogEntry[];
    onadd: (entry: CatalogEntry) => void;
  } = $props();

  const ORDER = ["trigger", "action", "transform", "flow", "code", "agent"];
  const LABELS: Record<string, string> = {
    trigger: "Triggers",
    action: "Actions",
    transform: "Transform",
    flow: "Flow",
    code: "Code",
    agent: "Agent",
  };

  const groups = $derived(
    ORDER.map((cat) => ({
      cat,
      label: LABELS[cat] ?? cat,
      entries: catalog.filter((e) => e.category === cat),
    })).filter((g) => g.entries.length > 0),
  );

  /** Payload type matched by the canvas drop handler in the editor page. */
  function dragstart(e: DragEvent, entry: CatalogEntry) {
    if (!e.dataTransfer) return;
    e.dataTransfer.setData("application/x-swervebuild-node", entry.type);
    e.dataTransfer.effectAllowed = "copy";
  }
</script>

<aside class="palette">
  <div class="palette-head mono-label">Add nodes — click or drag</div>
  <div class="groups">
    {#each groups as group (group.cat)}
      <div class="group">
        <div class="group-label" style="--cat-color: {categoryColor(group.cat)}">
          <span class="swatch"></span>
          {group.label}
        </div>
        {#each group.entries as entry (entry.type)}
          <button
            class="entry"
            title={entry.description}
            draggable="true"
            ondragstart={(e) => dragstart(e, entry)}
            onclick={() => onadd(entry)}
          >
            <span class="entry-icon" style="--cat-color: {categoryColor(entry.category)}">
              <Icon name={nodeIcon(entry.type)} size={14} />
            </span>
            <span class="entry-label">{entry.label}</span>
            <span class="entry-add"><Icon name="plus" size={12} /></span>
          </button>
        {/each}
      </div>
    {/each}
  </div>
</aside>

<style>
  .palette {
    display: flex;
    flex-direction: column;
    width: 208px;
    flex: none;
    border-right: 1px solid var(--border);
    background: var(--bg-sidebar);
    overflow: hidden;
  }

  .palette-head {
    padding: 0.85rem 1rem 0.5rem;
  }

  .groups {
    flex: 1;
    overflow-y: auto;
    padding: 0 0.6rem 1rem;
  }

  .group + .group {
    margin-top: 0.9rem;
  }

  .group-label {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    padding: 0.25rem 0.4rem;
    font-size: 0.6875rem;
    font-weight: 600;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--text-muted);
  }

  .swatch {
    width: 8px;
    height: 8px;
    border-radius: 3px;
    background: var(--cat-color);
  }

  .entry {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    width: 100%;
    padding: 0.42rem 0.5rem;
    border: 1px solid transparent;
    border-radius: var(--radius-sm);
    background: transparent;
    cursor: pointer;
    text-align: left;
    transition:
      background var(--dur-fast) var(--ease),
      border-color var(--dur-fast) var(--ease);
  }

  .entry:hover {
    background: var(--bg-hover);
    border-color: var(--border-subtle);
  }

  .entry:hover .entry-add {
    opacity: 1;
  }

  .entry-icon {
    display: grid;
    place-items: center;
    width: 24px;
    height: 24px;
    flex: none;
    border-radius: 7px;
    background: color-mix(in srgb, var(--cat-color) 15%, transparent);
    color: var(--cat-color);
  }

  .entry-label {
    flex: 1;
    font-size: 0.8125rem;
    color: var(--text-secondary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }

  .entry-add {
    opacity: 0;
    color: var(--text-faint);
    transition: opacity var(--dur-fast) var(--ease);
  }
</style>

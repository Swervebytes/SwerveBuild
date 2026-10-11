<script lang="ts">
  import { invoke } from "@tauri-apps/api/core";
  import type { ProjectCard } from "$lib/types";

  let { projectId, hideChatId = "" }: { projectId: string; hideChatId?: string } = $props();

  let card = $state<ProjectCard | null>(null);

  $effect(() => {
    const id = projectId;
    card = null;
    let cancelled = false;
    void invoke<ProjectCard>("get_project_card", { projectId: id })
      .then((next) => {
        if (!cancelled) card = next;
      })
      .catch(() => {
        if (!cancelled) card = null;
      });
    return () => {
      cancelled = true;
    };
  });

  const rows = $derived(
    (card?.chats ?? []).filter(
      (chat) => chat.id !== hideChatId && (chat.last_request || chat.open),
    ),
  );

  function outcomeLabel(outcome: string) {
    if (outcome === "finished") return "finished";
    if (outcome === "error") return "stopped on an error";
    return "no finished turn";
  }
</script>

{#if card}
  <section class="status" data-testid="project-status">
    <p class="kicker">Project status</p>
    <p class="git">
      {#if card.branch}
        <span class="branch">{card.branch}</span>
      {:else}
        <span>No git branch</span>
      {/if}
      {#if card.dirty.length === 0}
        <span class="quiet">· clean</span>
      {:else}
        <span class="quiet">
          · {card.dirty.length + card.dirty_more} changed
        </span>
      {/if}
    </p>
    {#if card.dirty.length > 0}
      <p class="dirty" title={card.dirty.join("\n")}>
        {card.dirty.join(", ")}{#if card.dirty_more > 0}&nbsp;(+{card.dirty_more} more){/if}
      </p>
    {/if}
    {#if rows.length === 0}
      <p class="quiet">No earlier turns in this project yet.</p>
    {:else}
      <ul>
        {#each rows as row (row.id)}
          <li>
            <span class="title">
              {row.title}
              {#if row.open}<span class="live" title="Open"></span>{/if}
            </span>
            {#if row.last_request}
              <span class="quiet">{outcomeLabel(row.outcome)}</span>
              <span class="request">{row.last_request}</span>
            {:else}
              <span class="quiet">open, no request yet</span>
            {/if}
          </li>
        {/each}
      </ul>
    {/if}
  </section>
{/if}

<style>
  .status {
    display: flex;
    flex-direction: column;
    gap: 0.3rem;
    padding: 0.7rem 0.8rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-sm, 6px);
    background: var(--bg-muted);
  }
  .kicker {
    margin: 0;
    font-size: 0.6875rem;
    letter-spacing: 0.04em;
    text-transform: uppercase;
    color: var(--text-faint);
  }
  .git,
  .dirty,
  .quiet,
  .request {
    margin: 0;
    font-size: 0.8125rem;
    line-height: 1.4;
  }
  .branch {
    font-family: var(--font-mono);
    font-size: 0.75rem;
  }
  .quiet {
    color: var(--text-muted);
  }
  .dirty,
  .request {
    color: var(--text-secondary);
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  ul {
    list-style: none;
    margin: 0.2rem 0 0;
    padding: 0;
    display: flex;
    flex-direction: column;
    gap: 0.45rem;
  }
  li {
    display: flex;
    flex-direction: column;
    gap: 0.1rem;
    min-width: 0;
  }
  .title {
    display: inline-flex;
    align-items: center;
    gap: 0.35rem;
    font-size: 0.8125rem;
    font-weight: 600;
  }
  .live {
    width: 7px;
    height: 7px;
    border-radius: 99px;
    background: var(--success, #3dd68c);
    flex: none;
  }
</style>

<script lang="ts">
  import { onMount } from "svelte";
  import { goto } from "$app/navigation";
  import { loadWorkspace } from "$lib/workspace";
  import EmptyState from "$lib/components/ui/EmptyState.svelte";

  let loading = $state(true);

  onMount(async () => {
    const workspace = await loadWorkspace();
    const latest = [...workspace.chats].sort(
      (a, b) => Number(b.updated_at) - Number(a.updated_at),
    )[0];

    if (latest) {
      await goto(`/chat/${latest.id}`);
      return;
    }

    loading = false;
  });
</script>

<div class="page">
  {#if loading}
    <p class="muted">Loading chats…</p>
  {:else}
    <section class="card">
      <EmptyState
        icon="chat"
        title="No chats yet"
        description="Add a project folder, then start a chat to build with Grok."
      >
        <a class="btn btn-primary" href="/projects">Go to Projects</a>
      </EmptyState>
    </section>
  {/if}
</div>

<style>
  .page {
    max-width: var(--reading-width);
    margin-inline: auto;
  }
  .muted {
    color: var(--text-muted);
  }
</style>

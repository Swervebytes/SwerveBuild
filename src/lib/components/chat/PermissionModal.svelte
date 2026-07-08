<script lang="ts">
  import { scale } from "svelte/transition";
  import { cubicOut } from "svelte/easing";
  import type { PermissionRequest } from "$lib/types";

  let {
    request,
    queueLength,
    isBackground = false,
    onrespond,
  }: {
    request: PermissionRequest & { chatTitle: string };
    queueLength: number;
    isBackground?: boolean;
    onrespond: (optionId: string) => void;
  } = $props();
</script>

<div class="overlay" role="presentation">
  <div
    class="modal"
    role="dialog"
    aria-modal="true"
    aria-labelledby="permission-title"
    transition:scale={{ duration: 150, start: 0.96, opacity: 0, easing: cubicOut }}
  >
    <h2 id="permission-title" class="title">Tool approval required</h2>
    <p class="chat">
      Chat: <strong>{request.chatTitle}</strong>
      {#if isBackground}<span class="bg"> (background)</span>{/if}
    </p>

    {#if request.params.toolCall?.title}
      <p class="tool">{request.params.toolCall.title}</p>
    {/if}

    <div class="actions">
      {#each request.params.options ?? [] as option}
        <button
          class="btn"
          class:btn-primary={option.kind === "allow_once" || option.kind === "allow_always"}
          type="button"
          onclick={() => onrespond(option.optionId)}
        >
          {option.name}
        </button>
      {/each}
    </div>

    {#if queueLength > 1}
      <p class="queue">{queueLength - 1} more pending</p>
    {/if}
  </div>
</div>

<style>
  .overlay {
    position: fixed;
    inset: 0;
    background: rgba(2, 4, 10, 0.6);
    backdrop-filter: blur(2px);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 100;
    padding: 1rem;
  }
  .modal {
    width: min(480px, 100%);
    background: var(--bg-surface);
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    box-shadow: var(--shadow-lg);
    padding: 1.25rem;
  }
  .title {
    font-size: 1rem;
    font-weight: 600;
    margin-bottom: 0.5rem;
  }
  .chat {
    font-size: 0.8125rem;
    color: var(--text-secondary);
    margin-bottom: 0.75rem;
  }
  .bg {
    color: var(--text-muted);
  }
  .tool {
    font-family: var(--font-mono);
    font-size: 0.8125rem;
    margin-bottom: 1rem;
    padding: 0.7rem 0.75rem;
    background: var(--bg-muted);
    border: 1px solid var(--border);
    border-radius: var(--radius-sm);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 0.5rem;
  }
  .queue {
    margin-top: 0.75rem;
    font-size: 0.75rem;
    color: var(--text-muted);
  }
</style>

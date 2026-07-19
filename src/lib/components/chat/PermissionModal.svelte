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

  let modalEl: HTMLDivElement | undefined = $state();
  let previouslyFocused: HTMLElement | null = null;

  // Escape picks the safe (non-allow) option rather than dismissing the dialog:
  // leaving the request unanswered would hang the waiting agent, and denying is
  // the conservative default. If the agent offered no reject option, Escape does
  // nothing — a decision is genuinely required.
  const denyOption = $derived(
    (request.params.options ?? []).find((o) => !o.kind?.startsWith("allow")) ?? null,
  );

  function focusable(): HTMLElement[] {
    if (!modalEl) return [];
    return Array.from(modalEl.querySelectorAll<HTMLElement>("button:not([disabled])"));
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      if (denyOption) {
        e.preventDefault();
        onrespond(denyOption.optionId);
      }
      return;
    }
    if (e.key !== "Tab") return;

    // Focus trap: Tab cycles inside the dialog so focus can never land on the
    // page behind a blocking approval prompt.
    const items = focusable();
    if (items.length === 0) return;
    const first = items[0];
    const last = items[items.length - 1];
    const active = document.activeElement as HTMLElement | null;
    const inside = !!active && !!modalEl?.contains(active);
    if (e.shiftKey && (!inside || active === first)) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && (!inside || active === last)) {
      e.preventDefault();
      first.focus();
    }
  }

  // Remember what had focus, and restore it when the dialog goes away.
  $effect(() => {
    previouslyFocused = document.activeElement as HTMLElement | null;
    return () => previouslyFocused?.focus?.();
  });

  // Move focus into the dialog on open, and again whenever the queue advances
  // to a different request (the buttons are replaced).
  $effect(() => {
    void request.requestId;
    queueMicrotask(() => focusable()[0]?.focus());
  });
</script>

<svelte:window onkeydown={onKeydown} />

<div class="overlay" role="presentation">
  <div
    class="modal"
    bind:this={modalEl}
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

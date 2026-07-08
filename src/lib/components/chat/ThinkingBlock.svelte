<script lang="ts">
  import { slide } from "svelte/transition";
  import Icon from "$lib/components/ui/Icon.svelte";

  let { content }: { content: string } = $props();

  let open = $state(false);
</script>

<div class="think">
  <button class="head" type="button" onclick={() => (open = !open)} aria-expanded={open}>
    <span class="ico"><Icon name="brain" size={14} /></span>
    <span class="label">Thinking</span>
    <span class="chev" class:open><Icon name="chevron-down" size={13} /></span>
  </button>
  {#if open}
    <div class="body" transition:slide={{ duration: 160 }}>{content}</div>
  {/if}
</div>

<style>
  .think {
    margin-left: calc(28px + 0.85rem);
    border-left: 2px solid var(--border);
    padding-left: 0.75rem;
  }
  .head {
    display: inline-flex;
    align-items: center;
    gap: 0.45rem;
    border: 0;
    background: transparent;
    color: var(--text-muted);
    cursor: pointer;
    padding: 0.25rem 0;
    font-size: 0.8125rem;
  }
  .head:hover {
    color: var(--text-secondary);
  }
  .ico {
    display: inline-flex;
  }
  .label {
    font-weight: 500;
  }
  .chev {
    display: inline-flex;
    transition: transform var(--dur) var(--ease);
  }
  .chev.open {
    transform: rotate(180deg);
  }
  .body {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    font-family: var(--font-mono);
    font-size: 0.78rem;
    line-height: 1.55;
    color: var(--text-faint);
    padding: 0.35rem 0 0.5rem;
  }
</style>

<script lang="ts">
  import Icon from "$lib/components/ui/Icon.svelte";

  let { title }: { title: string } = $props();
</script>

<div class="chip" role="status">
  <span class="ico"><Icon name="tool" size={13} /></span>
  <span class="title">{title}</span>
</div>

<style>
  .chip {
    display: inline-flex;
    align-items: center;
    gap: 0.5rem;
    max-width: 100%;
    margin-left: calc(28px + 0.85rem); /* align under message body gutter */
    padding: 0.4rem 0.65rem;
    border: 1px solid color-mix(in srgb, var(--sc-accent) 28%, var(--border));
    border-radius: var(--radius-sm);
    background: var(--sc-accent-tint);
    color: var(--text-secondary);
  }
  .ico {
    display: inline-flex;
    color: var(--sc-accent);
    flex: none;
  }
  .title {
    font-family: var(--font-mono);
    font-size: 0.75rem;
    color: var(--text-secondary);
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
  }
</style>

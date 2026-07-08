<script lang="ts">
  import Icon from "./Icon.svelte";
  import type { IconName } from "./icons";

  let {
    icon,
    title,
    description,
    children,
  }: {
    icon?: IconName;
    title: string;
    description?: string;
    children?: import("svelte").Snippet;
  } = $props();
</script>

<div class="empty">
  {#if icon}
    <div class="icon">
      <Icon name={icon} size={22} />
    </div>
  {/if}
  <h2 class="title">{title}</h2>
  {#if description}
    <p class="desc">{description}</p>
  {/if}
  {#if children}
    <div class="actions">{@render children()}</div>
  {/if}
</div>

<style>
  .empty {
    display: flex;
    flex-direction: column;
    align-items: center;
    text-align: center;
    gap: 0.5rem;
    padding: 2.5rem 1.5rem;
  }
  .icon {
    display: grid;
    place-items: center;
    width: 3rem;
    height: 3rem;
    border-radius: var(--radius);
    background: var(--sc-accent-tint);
    color: var(--sc-accent);
    margin-bottom: 0.25rem;
  }
  .title {
    font-size: 1rem;
    font-weight: 600;
    color: var(--text-primary);
  }
  .desc {
    font-size: 0.875rem;
    color: var(--text-muted);
    max-width: 42ch;
  }
  .actions {
    display: flex;
    gap: 0.5rem;
    margin-top: 0.75rem;
    flex-wrap: wrap;
    justify-content: center;
  }
</style>

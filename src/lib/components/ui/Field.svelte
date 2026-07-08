<script lang="ts">
  let {
    label,
    hint,
    row = false,
    children,
  }: {
    label: string;
    hint?: string;
    row?: boolean;
    children?: import("svelte").Snippet;
  } = $props();
</script>

<label class="field" class:row>
  <span class="field-label">{label}</span>
  {#if children}
    <div class="control">{@render children()}</div>
  {/if}
  {#if hint}
    <span class="field-hint">{hint}</span>
  {/if}
</label>

<style>
  .field {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
  }
  .field + :global(.field) {
    margin-top: 1rem;
  }
  .field.row {
    flex-direction: row;
    align-items: center;
    justify-content: space-between;
    gap: 1rem;
  }
  .field-label {
    font-size: 0.875rem;
    font-weight: 500;
    color: var(--text-primary);
  }
  .field-hint {
    font-size: 0.75rem;
    color: var(--text-muted);
  }
  .row .control {
    flex: none;
  }
</style>

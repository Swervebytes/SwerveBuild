<script lang="ts">
  import ModelsMenu from "$lib/components/models/ModelsMenu.svelte";
  import StatusPill from "$lib/components/ui/StatusPill.svelte";

  let {
    showModelPicker = false,
    modelId = null,
    modelSwitching = false,
    connected = false,
    onmodelchange,
  }: {
    showModelPicker?: boolean;
    modelId?: string | null;
    modelSwitching?: boolean;
    /** This chat's agent session is ready for the next message. */
    connected?: boolean;
    onmodelchange?: (id: string | null) => void;
  } = $props();
</script>

<div class="dock">
  <div class="dock-status">
    {#if connected}
      <StatusPill tone="success" label="Connected" />
    {:else}
      <StatusPill tone="warning" label="Connecting…" pulse />
    {/if}
  </div>

  <ModelsMenu
    sheetUp
    {showModelPicker}
    {modelId}
    {modelSwitching}
    {onmodelchange}
  />
</div>

<style>
  .dock {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 0.75rem;
    margin-bottom: 0.45rem;
  }
  .dock-status {
    display: flex;
    align-items: center;
    gap: 0.45rem;
    min-width: 0;
  }
</style>

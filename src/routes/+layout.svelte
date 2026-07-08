<script lang="ts">
  import "../app.css";
  import { onMount } from "svelte";
  import Sidebar from "$lib/components/Sidebar.svelte";
  import Titlebar from "$lib/components/Titlebar.svelte";
  import { theme } from "$lib/stores/theme.svelte";
  import { workspaceStore } from "$lib/stores/workspace.svelte";
  import { providerStore } from "$lib/stores/providers.svelte";

  let { children } = $props();

  onMount(() => {
    theme.init();
    workspaceStore.refresh();
    providerStore.load();
  });
</script>

<div class="app-shell">
  <Titlebar />
  <div class="body">
    <Sidebar />
    <main class="content">
      {@render children()}
    </main>
  </div>
</div>

<style>
  .app-shell {
    display: flex;
    flex-direction: column;
    height: 100vh;
    overflow: hidden;
  }

  .body {
    display: flex;
    flex: 1;
    min-height: 0;
    overflow: hidden;
  }

  .content {
    flex: 1;
    overflow: auto;
    padding: 1.5rem 2rem;
    background: var(--bg-app);
  }
</style>

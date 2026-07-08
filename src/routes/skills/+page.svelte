<script lang="ts">
  import { onMount } from "svelte";
  import { invoke } from "@tauri-apps/api/core";
  import Icon from "$lib/components/ui/Icon.svelte";
  import EmptyState from "$lib/components/ui/EmptyState.svelte";

  type Skill = {
    name: string;
    path: string;
    description: string;
    source: string;
  };

  let skills = $state<Skill[]>([]);
  let loading = $state(true);

  onMount(async () => {
    try {
      skills = await invoke<Skill[]>("list_skills");
    } finally {
      loading = false;
    }
  });
</script>

<div class="page">
  <header class="page-header">
    <h1 class="page-title">Skills</h1>
    <p class="page-subtitle">
      <span class="badge badge-muted"><Icon name="agent" size={12} /> Grok</span>
      Installed skills from <span class="mono">~/.grok</span>
    </p>
  </header>

  {#if loading}
    <p class="muted">Loading skills…</p>
  {:else if skills.length === 0}
    <section class="card">
      <EmptyState
        icon="skills"
        title="No skills found"
        description="Skills installed under ~/.grok/skills or ~/.grok/bundled/skills will appear here."
      />
    </section>
  {:else}
    <div class="skill-grid">
      {#each skills as skill (skill.path)}
        <article class="card skill-card">
          <div class="skill-header">
            <span class="skill-icon"><Icon name="skills" size={16} /></span>
            <h2 class="skill-name">{skill.name}</h2>
            <span class="badge badge-muted">{skill.source}</span>
          </div>
          <p class="skill-desc">{skill.description}</p>
          <p class="skill-path">{skill.path}</p>
        </article>
      {/each}
    </div>
  {/if}
</div>

<style>
  .page {
    max-width: 760px;
    margin-inline: auto;
  }
  .page-header {
    margin-bottom: 1.25rem;
  }
  .page-subtitle {
    display: flex;
    align-items: center;
    gap: 0.5rem;
    flex-wrap: wrap;
  }
  .badge :global(svg) {
    margin-right: 0.15rem;
  }
  .mono {
    font-family: var(--font-mono);
    font-size: 0.8125rem;
    color: var(--text-faint);
  }

  .skill-grid {
    display: flex;
    flex-direction: column;
    gap: 0.7rem;
  }
  .skill-card {
    padding: 1rem 1.15rem;
  }
  .skill-header {
    display: flex;
    align-items: center;
    gap: 0.55rem;
    margin-bottom: 0.4rem;
  }
  .skill-icon {
    display: inline-flex;
    color: var(--sc-accent);
    flex: none;
  }
  .skill-name {
    font-size: 0.9375rem;
    font-weight: 600;
    font-family: var(--font-mono);
    flex: 1;
    min-width: 0;
  }
  .skill-desc {
    font-size: 0.875rem;
    color: var(--text-secondary);
    margin-bottom: 0.5rem;
    line-height: 1.5;
  }
  .skill-path {
    font-size: 0.75rem;
    font-family: var(--font-mono);
    color: var(--text-faint);
    word-break: break-all;
  }
  .muted {
    color: var(--text-muted);
    font-size: 0.875rem;
  }
</style>

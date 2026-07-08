<script lang="ts">
  import Icon from "$lib/components/ui/Icon.svelte";
</script>

<div class="page">
  <header class="page-header">
    <h1 class="page-title">Terminal</h1>
    <p class="page-subtitle">Direct Grok Build CLI access</p>
  </header>

  <section class="card terminal">
    <div class="bar">
      <span class="dot"></span>
      <span class="dot"></span>
      <span class="dot"></span>
      <span class="label mono">grok</span>
    </div>
    <div class="body">
      <div class="glyph"><Icon name="terminal" size={22} /></div>
      <p class="line"><span class="prompt">$</span> grok --help</p>
      <h2 class="soon">Embedded terminal coming soon</h2>
      <p class="hint">This view will host a PTY for running Grok commands without leaving the app.</p>
    </div>
  </section>
</div>

<style>
  .page {
    max-width: 820px;
    height: calc(100vh - var(--titlebar-height) - 3rem);
    display: flex;
    flex-direction: column;
    margin-inline: auto;
  }
  .page-header {
    margin-bottom: 1rem;
    flex-shrink: 0;
  }
  .terminal {
    flex: 1;
    display: flex;
    flex-direction: column;
    padding: 0;
    overflow: hidden;
    min-height: 280px;
  }
  .bar {
    display: flex;
    align-items: center;
    gap: 0.4rem;
    padding: 0.6rem 0.9rem;
    border-bottom: 1px solid var(--border);
    background: var(--bg-surface-2);
  }
  .dot {
    width: 0.65rem;
    height: 0.65rem;
    border-radius: 50%;
    background: var(--border-strong);
  }
  .dot:nth-child(1) {
    background: #ff6b6b;
  }
  .dot:nth-child(2) {
    background: #f5b656;
  }
  .dot:nth-child(3) {
    background: #4dd2c0;
  }
  .label {
    margin-left: 0.5rem;
    font-size: 0.75rem;
    color: var(--text-muted);
  }
  .mono {
    font-family: var(--font-mono);
  }
  .body {
    flex: 1;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 0.5rem;
    padding: 2rem;
    text-align: center;
    font-family: var(--font-mono);
  }
  .glyph {
    display: grid;
    place-items: center;
    width: 3rem;
    height: 3rem;
    border-radius: var(--radius);
    background: var(--sc-accent-tint);
    color: var(--sc-accent);
    margin-bottom: 0.5rem;
  }
  .line {
    font-size: 0.8125rem;
    color: var(--text-secondary);
  }
  .prompt {
    color: var(--sc-accent);
    margin-right: 0.4rem;
  }
  .soon {
    font-family: var(--font-sans);
    font-size: 1rem;
    font-weight: 600;
    color: var(--text-primary);
    margin-top: 0.4rem;
  }
  .hint {
    font-family: var(--font-sans);
    font-size: 0.875rem;
    color: var(--text-muted);
    max-width: 42ch;
  }
</style>

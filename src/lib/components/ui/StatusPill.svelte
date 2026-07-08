<script lang="ts">
  let {
    tone = "muted",
    label,
    pulse = false,
  }: {
    tone?: "success" | "warning" | "danger" | "accent" | "muted";
    label: string;
    pulse?: boolean;
  } = $props();
</script>

<span class="pill {tone}">
  <span class="dot" class:pulse></span>
  <span class="label">{label}</span>
</span>

<style>
  .pill {
    display: inline-flex;
    align-items: center;
    gap: 0.4rem;
    padding: 0.2rem 0.6rem 0.2rem 0.5rem;
    border-radius: var(--radius-pill);
    font-size: 0.75rem;
    font-weight: 500;
    border: 1px solid var(--border);
    background: var(--bg-muted);
    color: var(--text-secondary);
    white-space: nowrap;
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: currentColor;
    flex: none;
  }
  .label {
    line-height: 1;
  }

  .success {
    color: var(--success);
    background: var(--success-tint);
    border-color: transparent;
  }
  .warning {
    color: var(--warning);
    background: var(--warning-tint);
    border-color: transparent;
  }
  .danger {
    color: var(--danger);
    background: var(--danger-tint);
    border-color: transparent;
  }
  .accent {
    color: var(--sc-accent);
    background: var(--sc-accent-tint);
    border-color: transparent;
  }
  .muted {
    color: var(--text-muted);
  }

  .pulse {
    position: relative;
  }
  .pulse::after {
    content: "";
    position: absolute;
    inset: -3px;
    border-radius: 50%;
    background: currentColor;
    opacity: 0.5;
    animation: pulse-ring 1.8s var(--ease) infinite;
  }
  @keyframes pulse-ring {
    0% {
      transform: scale(0.7);
      opacity: 0.5;
    }
    100% {
      transform: scale(2.2);
      opacity: 0;
    }
  }
</style>

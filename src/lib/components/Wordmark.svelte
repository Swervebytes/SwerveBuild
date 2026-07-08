<script lang="ts">
  // Swerve Build wordmark: "Swerve" in foreground + "Build" in the signature gradient.
  let {
    suffix = "Build",
    size = "md",
  }: { suffix?: string; size?: "sm" | "md" | "lg" } = $props();
</script>

<span class="wordmark {size}">
  <span class="swerve">Swerve</span><span class="suffix sc-gradient-text">{suffix}</span>
</span>

<style>
  .wordmark {
    display: inline-flex;
    align-items: baseline;
    font-weight: 800;
    letter-spacing: -0.02em;
    line-height: 1;
    user-select: none;
  }
  .swerve {
    color: var(--text-primary);
  }
  .sm {
    font-size: 0.9375rem;
  }
  .md {
    font-size: 1.125rem;
  }
  .lg {
    font-size: 1.75rem;
  }
</style>
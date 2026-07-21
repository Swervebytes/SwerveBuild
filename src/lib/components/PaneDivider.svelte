<script lang="ts">
  // A thin vertical drag handle between two panes (S13f). Reports the pointer's
  // clientX during a drag; the parent turns that into a width. Uses pointer
  // capture so the drag keeps tracking even if the cursor leaves the handle.
  let {
    onbegin,
    onmove,
    onend,
    ariaLabel = "Resize",
  }: {
    onbegin?: () => void;
    onmove?: (clientX: number) => void;
    onend?: () => void;
    ariaLabel?: string;
  } = $props();

  let active = $state(false);

  function down(e: PointerEvent) {
    e.preventDefault();
    active = true;
    const el = e.currentTarget as HTMLElement;
    el.setPointerCapture?.(e.pointerId);
    document.body.classList.add("resizing-col");
    onbegin?.();

    const move = (ev: PointerEvent) => onmove?.(ev.clientX);
    const up = (ev: PointerEvent) => {
      active = false;
      document.body.classList.remove("resizing-col");
      el.releasePointerCapture?.(ev.pointerId);
      el.removeEventListener("pointermove", move);
      el.removeEventListener("pointerup", up);
      el.removeEventListener("pointercancel", up);
      onend?.();
    };
    el.addEventListener("pointermove", move);
    el.addEventListener("pointerup", up);
    el.addEventListener("pointercancel", up);
  }
</script>

<div
  class="divider"
  class:active
  role="separator"
  aria-orientation="vertical"
  aria-label={ariaLabel}
  tabindex="-1"
  onpointerdown={down}
></div>

<style>
  .divider {
    flex: 0 0 auto;
    width: 5px;
    align-self: stretch;
    cursor: col-resize;
    background: transparent;
    /* Widen the hit area without consuming layout beyond the 5px strip. */
    position: relative;
    z-index: 6;
    transition: background var(--dur-fast) var(--ease);
  }
  .divider::after {
    content: "";
    position: absolute;
    inset: 0 2px;
    border-radius: 2px;
  }
  .divider:hover::after,
  .divider.active::after {
    background: var(--sc-accent);
    opacity: 0.55;
  }
</style>

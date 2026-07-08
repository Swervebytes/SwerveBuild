<script lang="ts">
  import { ICON_PATHS, type IconName } from "./icons";

  let {
    name,
    size = 16,
    stroke = 2,
  }: { name: IconName; size?: number; stroke?: number } = $props();
</script>

<svg
  xmlns="http://www.w3.org/2000/svg"
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill="none"
  stroke="currentColor"
  stroke-width={stroke}
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
  focusable="false"
>
  {@html ICON_PATHS[name]}
</svg>

<style>
  svg {
    display: block;
    flex: none;
  }
</style>

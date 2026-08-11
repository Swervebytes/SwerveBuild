/** P4.1 (audit E): text color that stays readable on the provider accent.
 * `.btn-accent` used to hardcode near-black, which disappears on dark accents.
 * Plain module (no runes) so vitest covers it. */
export function accentForeground(hex: string): string {
  const m = /^#?([0-9a-f]{6})$/i.exec(hex.trim());
  if (!m) return "#04060d";
  const n = parseInt(m[1], 16);
  const r = (n >> 16) & 255;
  const g = (n >> 8) & 255;
  const b = n & 255;
  // WCAG-ish relative luminance, cheap linearization.
  const lum = (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255;
  return lum > 0.45 ? "#04060d" : "#ffffff";
}

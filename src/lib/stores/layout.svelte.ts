// Resizable sidebar width (S13f). Drives the `--sidebar-width` CSS var that both
// the sidebar and the titlebar rail already consume, so setting it here resizes
// both. Persisted across launches.

const KEY = "swervebuild.sidebarWidth";
const MIN = 200;
const MAX = 460;
const DEFAULT = 264; // matches the app.css :root default

let width = $state(DEFAULT);

function apply() {
  if (typeof document !== "undefined") {
    document.documentElement.style.setProperty("--sidebar-width", `${width}px`);
  }
}

export const layout = {
  get sidebarWidth() {
    return width;
  },

  /** Clamp + apply + persist. `px` is the desired sidebar width (e.g. the drag
   *  handle's clientX, since the handle sits at the sidebar's right edge). */
  setSidebarWidth(px: number) {
    const cap = typeof window !== "undefined" ? Math.min(MAX, window.innerWidth * 0.4) : MAX;
    width = Math.max(MIN, Math.min(cap, Math.round(px)));
    apply();
    try {
      localStorage.setItem(KEY, String(width));
    } catch {
      /* ignore */
    }
  },

  init() {
    try {
      const v = Number(localStorage.getItem(KEY));
      if (Number.isFinite(v) && v >= MIN && v <= MAX) width = v;
    } catch {
      /* ignore */
    }
    apply();
  },
};

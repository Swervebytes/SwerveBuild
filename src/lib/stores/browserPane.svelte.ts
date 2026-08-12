// Docked debug-browser pane (S13d).
//
// The actual page is a NATIVE child webview (label `swerve-debug`) that Tauri
// floats over the reserved area rendered by BrowserPane.svelte. This store only
// holds the human-facing UI state (open/loading/url/message) and talks to the
// Rust `browser_pane_*` commands. When closed, the webview is parked offscreen
// by Rust but stays alive, so the agent's browser_* MCP tools keep working.

import { invoke } from "@tauri-apps/api/core";

let open = $state(false);
let url = $state("");
let loading = $state(false);
let message = $state<string | null>(null);

// S13f: drag-to-resize. `width` is the pane's px width (persisted). `dragging`
// parks the native webview mid-drag (a native view can't smoothly track a drag
// and would swallow pointer events); `realignTick` re-pushes bounds on drag end.
const WIDTH_KEY = "swervebuild.browserPaneWidth";
const WIDTH_MIN = 340;
let width = $state(520);
let dragging = $state(false);
let realignTick = $state(0);

function widthMax() {
  return typeof window !== "undefined" ? Math.max(WIDTH_MIN, window.innerWidth - 360) : 1100;
}

function persistPark() {
  // Fire-and-forget: keep the native webview off-screen while the dock is shut.
  void invoke("browser_pane_park").catch(() => {
    /* pane not created yet / non-Tauri preview */
  });
}

async function nav(action: "back" | "forward" | "reload") {
  loading = true;
  message = null;
  try {
    const res = await invoke<{ ok: boolean; url?: string }>("browser_pane_nav", {
      action,
    });
    if (res?.url) url = res.url;
  } catch (e) {
    message = String(e);
  } finally {
    loading = false;
  }
}

export const browserPane = {
  get open() {
    return open;
  },
  get url() {
    return url;
  },
  get loading() {
    return loading;
  },
  get message() {
    return message;
  },
  get width() {
    return width;
  },
  get dragging() {
    return dragging;
  },
  get realignTick() {
    return realignTick;
  },

  openPane() {
    open = true;
    // Bounds are pushed by BrowserPane.svelte once it mounts.
  },

  /** Restore the persisted pane width on boot. */
  initWidth() {
    try {
      const v = Number(localStorage.getItem(WIDTH_KEY));
      if (Number.isFinite(v) && v >= WIDTH_MIN) width = Math.min(v, widthMax());
    } catch {
      /* ignore */
    }
  },

  /** Set the pane width (px), clamped + persisted. */
  setWidth(px: number) {
    width = Math.max(WIDTH_MIN, Math.min(widthMax(), Math.round(px)));
    try {
      localStorage.setItem(WIDTH_KEY, String(width));
    } catch {
      /* ignore */
    }
  },

  /** A divider drag started — park the webview so the drag is smooth. */
  beginResize() {
    dragging = true;
    persistPark();
  },

  /** Drag ended — realign the webview to the final reserved rect. */
  endResize() {
    dragging = false;
    realignTick += 1;
  },

  closePane() {
    open = false;
    persistPark();
  },

  toggle() {
    if (open) browserPane.closePane();
    else browserPane.openPane();
  },

  /** Align the native webview to the reserved viewport rect (logical px). */
  async setBounds(x: number, y: number, width: number, height: number) {
    try {
      await invoke("browser_pane_set_bounds", { x, y, width, height });
    } catch (e) {
      message = String(e);
    }
  },

  /** Park offscreen without closing (e.g. a modal is covering the app). */
  park() {
    persistPark();
  },

  /**
   * The pane navigated (agent drove `browser_open`, or a human nav) — surfaced
   * by the Rust `browser-pane-activity` event. Auto-open the dock so the human
   * sees what the agent is doing, and reflect the landed URL.
   */
  onActivity(nextUrl?: string) {
    if (nextUrl) url = nextUrl;
    open = true;
  },

  /** Navigate the pane to a localhost URL (loopback-only, enforced in Rust). */
  async navigate(next: string) {
    const target = next.trim();
    if (!target) return;
    loading = true;
    message = null;
    try {
      const res = await invoke<{ ok: boolean; url?: string; title?: string }>(
        "browser_pane_open",
        { url: target },
      );
      url = res?.url || target;
      if (res && res.ok === false) message = "load timed out. Check the URL / server";
    } catch (e) {
      // Rust errors arrive as "…: message"; keep the human-readable tail.
      message = String(e).replace(/^.*?:\s*/, "");
    } finally {
      loading = false;
    }
  },

  back() {
    return nav("back");
  },
  forward() {
    return nav("forward");
  },
  reload() {
    return nav("reload");
  },
};

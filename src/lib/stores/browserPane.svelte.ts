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

  openPane() {
    open = true;
    // Bounds are pushed by BrowserPane.svelte once it mounts.
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
      if (res && res.ok === false) message = "load timed out — check the URL / server";
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

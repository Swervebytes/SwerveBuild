// Theme store (Svelte 5 runes). The boot script in app.html already sets
// document.documentElement.dataset.theme before first paint; this store keeps
// the user's preference in sync so the Settings selector reflects/controls it.

export type ThemePref = "system" | "light" | "dark";

const KEY = "swervebuild.theme";

let pref = $state<ThemePref>("dark");

function resolve(p: ThemePref): "light" | "dark" {
  if (p === "system") {
    return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
  }
  return p;
}

function apply() {
  if (typeof document !== "undefined") {
    document.documentElement.dataset.theme = resolve(pref);
  }
}

export const theme = {
  get pref() {
    return pref;
  },
  get resolved() {
    return resolve(pref);
  },
  set(next: ThemePref) {
    pref = next;
    try {
      localStorage.setItem(KEY, next);
    } catch {
      /* ignore */
    }
    apply();
  },
  init() {
    try {
      const stored = (localStorage.getItem(KEY) ??
        localStorage.getItem("swervegrok.theme")) as ThemePref | null;
      if (stored === "system" || stored === "light" || stored === "dark") {
        pref = stored;
      }
    } catch {
      /* ignore */
    }
    apply();
    try {
      window
        .matchMedia("(prefers-color-scheme: dark)")
        .addEventListener("change", () => {
          if (pref === "system") apply();
        });
    } catch {
      /* ignore */
    }
  },
};

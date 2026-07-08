// Provider store (Svelte 5 runes). Backs the chat-header ProviderPicker and the
// Settings > Providers section, and drives the per-provider --sc-accent.
//
// The Rust provider commands land in a later phase; until then load() falls back
// to a built-in Grok view so the app keeps working and the accent stays correct.

import { invoke } from "@tauri-apps/api/core";
import type { ProviderView } from "$lib/types";

const FALLBACK_GROK: ProviderView = {
  id: "grok",
  label: "Grok",
  kind: "acp",
  command: null,
  args: ["agent", "stdio"],
  env: [],
  accent: "#6cb5ff",
  model: null,
  base_url: null,
  builtin: true,
  available: true,
  active: true,
};

let providers = $state<ProviderView[]>([FALLBACK_GROK]);
let active = $state<ProviderView>(FALLBACK_GROK);

function applyAccent() {
  if (typeof document !== "undefined") {
    document.documentElement.style.setProperty("--sc-accent", active.accent);
  }
}

export const providerStore = {
  get all() {
    return providers;
  },
  get active() {
    return active;
  },
  async load() {
    try {
      const list = await invoke<ProviderView[]>("list_providers");
      if (list.length > 0) {
        providers = list;
        active = list.find((p) => p.active) ?? list[0];
      }
    } catch {
      // Provider backend not available yet — keep the Grok fallback.
      providers = [FALLBACK_GROK];
      active = FALLBACK_GROK;
    }
    applyAccent();
  },
  async setActive(id: string, model?: string) {
    try {
      active = await invoke<ProviderView>("set_active_provider", { id, model: model ?? null });
      providers = await invoke<ProviderView[]>("list_providers");
    } catch {
      // ignore — backend not ready
    }
    applyAccent();
  },
};

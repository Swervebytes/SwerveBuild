// Automations state (Svelte 5 runes). Mirrors workspace.svelte.ts: one reactive
// source read by the Automations page and the Sidebar failure badge. Degrades to
// empty on backend errors rather than throwing.

import { invoke } from "@tauri-apps/api/core";
import type { Automation, RunRecord } from "$lib/types";

let automations = $state<Automation[]>([]);
let paused = $state(false);
let failureCount = $state(0);
let loaded = $state(false);

export const automationsStore = {
  get automations() {
    return automations;
  },
  get paused() {
    return paused;
  },
  get failureCount() {
    return failureCount;
  },
  get loaded() {
    return loaded;
  },
  async refresh() {
    try {
      automations = await invoke<Automation[]>("list_automations");
    } catch {
      automations = [];
    }
    try {
      paused = await invoke<boolean>("get_automations_paused");
    } catch {
      paused = false;
    }
    await this.refreshFailures();
    loaded = true;
  },
  async refreshFailures() {
    try {
      failureCount = await invoke<number>("automation_failure_count");
    } catch {
      failureCount = 0;
    }
  },
  async save(automation: Automation): Promise<Automation> {
    const saved = await invoke<Automation>("save_automation", { automation });
    await this.refresh();
    return saved;
  },
  async remove(id: string) {
    await invoke("delete_automation", { id });
    await this.refresh();
  },
  async runNow(automationId: string): Promise<string> {
    return await invoke<string>("run_automation_now", { automationId });
  },
  async cancel(runId: string) {
    await invoke("cancel_run", { runId });
  },
  async setPaused(value: boolean) {
    await invoke("set_automations_paused", { paused: value });
    paused = value;
  },
  async runs(automationId: string): Promise<RunRecord[]> {
    try {
      return await invoke<RunRecord[]>("list_automation_runs", { automationId });
    } catch {
      return [];
    }
  },
  async log(automationId: string, runId: string): Promise<string> {
    try {
      return await invoke<string>("read_run_log", { automationId, runId });
    } catch {
      return "";
    }
  },
  async markSeen(automationId: string, runIds: string[]) {
    if (runIds.length === 0) return;
    try {
      await invoke("mark_runs_seen", { automationId, runIds });
    } catch {
      /* ignore */
    }
    await this.refreshFailures();
  },
};

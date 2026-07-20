// Typed bridge to the workflow engine commands. Shapes mirror the Rust serde
// models exactly (snake_case) — the same JSON that lives on disk.

import { invoke } from "@tauri-apps/api/core";

export type WfPermissions = {
  network: { enabled: boolean; hosts: string[]; private_ips: boolean };
  code: boolean;
  fs: { read: string[]; write: string[] };
  agent: boolean;
};

export type WfSettings = {
  timeout_secs: number;
  overlap: "skip" | "replace";
  min_interval_secs: number;
  keep_runs: number;
  capture: "none" | "sample" | "full";
  max_items_per_port: number;
};

export type WfOnError = "stop" | "skip" | "branch";

export type WfNode = {
  id: string;
  type: string;
  type_version: number;
  name: string;
  position: [number, number];
  disabled: boolean;
  notes: string;
  on_error: WfOnError;
  retry: { attempts: number; backoff_secs: number[] } | null;
  params: Record<string, unknown>;
};

export type WfConnection = { from: string; out: string; to: string; in: string };

export type WorkflowDoc = {
  version: number;
  id: string;
  name: string;
  enabled: boolean;
  project_id: string | null;
  settings: WfSettings;
  permissions: WfPermissions;
  nodes: WfNode[];
  connections: WfConnection[];
  state: {
    last_fired_at: number | null;
    last_run_id: string | null;
    last_status: string | null;
    trigger: Record<string, unknown>;
  };
  created_at: string;
  updated_at: string;
};

export type CatalogPort = { name: string; label: string };

export type CatalogEntry = {
  type: string;
  type_version: number;
  label: string;
  category: "trigger" | "flow" | "transform" | "action" | "code" | "agent";
  description: string;
  inputs: CatalogPort[];
  outputs: CatalogPort[];
  needs: { network: boolean; code: boolean; fs_read: boolean; fs_write: boolean; agent: boolean };
  is_trigger: boolean;
  secrets_ok: boolean;
};

export type WfNodeRunStatus = "success" | "error" | "skipped" | "cancelled";
export type WfRunStatus = "queued" | "running" | "success" | "error" | "cancelled" | "timeout";

export type WfNodeRunSummary = {
  node_id: string;
  name: string;
  status: WfNodeRunStatus;
  items_in: number;
  items_out: number;
  duration_ms: number;
  attempts: number;
  error?: { kind: string; message: string; item_index?: number } | null;
  warning?: string | null;
};

export type WfCapturedPort = { items: unknown[]; total: number; truncated: boolean };

export type WfRunRecord = {
  id: string;
  workflow_id: string;
  workflow_name: string;
  trigger: { kind: string; reason: string; node_id: string };
  status: WfRunStatus;
  started_at: string;
  finished_at: string | null;
  error?: { node_id: string; node_name: string; kind: string; message: string; item_index?: number } | null;
  nodes: WfNodeRunSummary[];
  data: Record<string, Record<string, WfCapturedPort>>;
  seen: boolean;
};

export type WfValidation = {
  errors: { node_id: string | null; message: string }[];
  warnings: { node_id: string | null; message: string }[];
};

// Event payloads (serde-tagged with `type`).
export type WfNodeStarted = { workflow_id: string; run_id: string; node_id: string; name: string };
export type WfNodeFinished = {
  workflow_id: string;
  run_id: string;
  node_id: string;
  name: string;
  status: WfNodeRunStatus;
  items_in: number;
  items_out: number;
  duration_ms: number;
  error?: string | null;
};
export type WfRunFinished = { workflow_id: string; run_id: string; status: WfRunStatus; error?: string | null };
export type WfRunLog = { workflow_id: string; run_id: string; node_id: string; level: string; message: string };

export const wfApi = {
  list: () => invoke<WorkflowDoc[]>("workflows_list"),
  get: (id: string) => invoke<WorkflowDoc | null>("workflow_get", { id }),
  save: (workflow: WorkflowDoc) => invoke<WorkflowDoc>("workflow_save", { workflow }),
  remove: (id: string) => invoke<void>("workflow_delete", { id }),
  validate: (workflow: WorkflowDoc) => invoke<WfValidation>("workflow_validate", { workflow }),
  catalog: () => invoke<CatalogEntry[]>("workflow_node_catalog"),
  runNow: (id: string, triggerNodeId?: string) =>
    invoke<string>("workflow_run_now", { id, triggerNodeId: triggerNodeId ?? null }),
  cancelRun: (runId: string) => invoke<void>("workflow_cancel_run", { runId }),
  runs: (workflowId: string) => invoke<WfRunRecord[]>("workflow_runs", { workflowId }),
  runDetail: (workflowId: string, runId: string) =>
    invoke<WfRunRecord | null>("workflow_run_detail", { workflowId, runId }),
  getPaused: () => invoke<boolean>("workflows_get_paused"),
  setPaused: (paused: boolean) => invoke<void>("workflows_set_paused", { paused }),
  secretNames: () => invoke<string[]>("workflow_secret_names"),
  secretSet: (name: string, value: string) => invoke<void>("workflow_secret_set", { name, value }),
  secretDelete: (name: string) => invoke<void>("workflow_secret_delete", { name }),
};

/** True when running inside the Tauri shell (vs a plain browser dev preview). */
export function inTauri(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

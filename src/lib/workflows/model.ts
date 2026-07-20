// Editor-side knowledge about node types: colors, icons, param forms, and
// factories. Ports/needs/labels come from the Rust catalog (single source of
// truth); this file adds the presentation layer the canvas needs.

import type { IconName } from "$lib/components/ui/icons";
import type { CatalogEntry, WfNode, WfRunStatus, WorkflowDoc } from "./api";

// Category accents drawn from the SwerveCreate gradient family.
export const CATEGORY_COLOR: Record<string, string> = {
  trigger: "#4dd2c0",
  flow: "#f5b656",
  transform: "#6cb5ff",
  action: "#8b5cf6",
  code: "#4cc9f0",
  agent: "#e8452c",
};

export const NODE_ICON: Record<string, IconName> = {
  "trigger.manual": "play",
  "trigger.schedule": "clock",
  "trigger.git": "git-branch",
  "trigger.file": "file",
  "http.request": "globe",
  "transform.set": "pencil",
  "flow.if": "split",
  "flow.merge": "merge",
  "code.js": "code",
  "agent.run": "agent",
  "file.read": "file",
  "file.write": "file",
  "util.wait": "hourglass",
};

export function nodeIcon(type: string): IconName {
  return NODE_ICON[type] ?? "tool";
}

export function categoryColor(category: string): string {
  return CATEGORY_COLOR[category] ?? "#6cb5ff";
}

// ------------------------------------------------------------- param forms

export type FieldKind =
  | "text"
  | "textarea"
  | "expression"
  | "number"
  | "select"
  | "toggle"
  | "keyvalue"
  | "code"
  | "conditions"
  | "ops"
  | "json";

export type Field = {
  key: string;
  label: string;
  kind: FieldKind;
  placeholder?: string;
  help?: string;
  options?: { value: string | number; label: string }[];
  /** Render only when this predicate over the params passes. */
  when?: (params: Record<string, unknown>) => boolean;
};

const METHODS = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD"].map((m) => ({ value: m, label: m }));

export const PARAM_FORMS: Record<string, Field[]> = {
  "trigger.manual": [],
  "trigger.schedule": [
    {
      key: "every",
      label: "Runs",
      kind: "select",
      options: [
        { value: "interval", label: "On an interval" },
        { value: "daily", label: "Every day" },
        { value: "weekly", label: "Every week" },
      ],
    },
    {
      key: "interval_minutes",
      label: "Every N minutes",
      kind: "number",
      help: "Minimum 15 minutes.",
      when: (p) => p.every === "interval",
    },
    { key: "hour", label: "Hour (0 to 23)", kind: "number", when: (p) => p.every !== "interval" },
    { key: "minute", label: "Minute", kind: "number", when: (p) => p.every !== "interval" },
    {
      key: "weekday",
      label: "Day",
      kind: "select",
      when: (p) => p.every === "weekly",
      options: [
        { value: 0, label: "Sunday" },
        { value: 1, label: "Monday" },
        { value: 2, label: "Tuesday" },
        { value: 3, label: "Wednesday" },
        { value: 4, label: "Thursday" },
        { value: 5, label: "Friday" },
        { value: 6, label: "Saturday" },
      ],
    },
  ],
  "trigger.git": [
    { key: "cwd", label: "Repository folder", kind: "text", placeholder: "E:\\MyProject" },
    { key: "branch", label: "Branch", kind: "text", placeholder: "Blank for the current branch" },
  ],
  "trigger.file": [
    { key: "path", label: "File or folder to watch", kind: "text", placeholder: "E:\\MyProject\\notes" },
    { key: "glob", label: "Name filter", kind: "text", placeholder: "*.md" },
  ],
  "http.request": [
    { key: "method", label: "Method", kind: "select", options: METHODS },
    {
      key: "url",
      label: "URL",
      kind: "expression",
      placeholder: "https://api.example.com/{{ $json.path }}",
    },
    { key: "query", label: "Query params", kind: "keyvalue" },
    { key: "headers", label: "Headers", kind: "keyvalue", help: "Values may use {{ $secret(\"name\") }}." },
    {
      key: "body_type",
      label: "Body",
      kind: "select",
      options: [
        { value: "none", label: "None" },
        { value: "json", label: "JSON" },
        { value: "text", label: "Text" },
        { value: "form", label: "Form" },
      ],
    },
    { key: "body", label: "Body content", kind: "json", when: (p) => p.body_type === "json" || p.body_type === "form" },
    { key: "body", label: "Body content", kind: "textarea", when: (p) => p.body_type === "text" },
    { key: "timeout_secs", label: "Timeout seconds", kind: "number" },
  ],
  "transform.set": [{ key: "ops", label: "Field operations", kind: "ops" }],
  "flow.if": [
    {
      key: "combine",
      label: "Combine",
      kind: "select",
      options: [
        { value: "and", label: "All conditions (and)" },
        { value: "or", label: "Any condition (or)" },
      ],
    },
    { key: "conditions", label: "Conditions", kind: "conditions" },
  ],
  "flow.merge": [
    {
      key: "mode",
      label: "Mode",
      kind: "select",
      options: [
        { value: "append", label: "Append A then B" },
        { value: "zip", label: "Zip pairs by position" },
      ],
    },
  ],
  "code.js": [
    {
      key: "mode",
      label: "Run",
      kind: "select",
      options: [
        { value: "all_items", label: "Once for all items" },
        { value: "per_item", label: "Once per item" },
      ],
    },
    {
      key: "code",
      label: "JavaScript",
      kind: "code",
      help: "All items mode gets items, returns an array. Per item mode gets item and index.",
    },
    { key: "timeout_secs", label: "Timeout seconds", kind: "number" },
  ],
  "agent.run": [
    { key: "prompt", label: "Prompt", kind: "expression", placeholder: "Summarize {{ $json.text }}" },
    { key: "cwd", label: "Project folder", kind: "text", placeholder: "E:\\MyProject" },
    { key: "model", label: "Model", kind: "text", placeholder: "Blank for the default model" },
    { key: "max_turns", label: "Max turns", kind: "number" },
    { key: "timeout_secs", label: "Timeout seconds", kind: "number" },
    { key: "web_search", label: "Allow web search", kind: "toggle" },
  ],
  "file.read": [{ key: "path", label: "File path", kind: "expression", placeholder: "E:\\data\\{{ $json.name }}.txt" }],
  "file.write": [
    { key: "path", label: "File path", kind: "expression" },
    { key: "content", label: "Content", kind: "expression", placeholder: "{{ $json.text }}" },
    {
      key: "mode",
      label: "Mode",
      kind: "select",
      options: [
        { value: "overwrite", label: "Overwrite" },
        { value: "append", label: "Append" },
      ],
    },
  ],
  "util.wait": [{ key: "seconds", label: "Seconds", kind: "number" }],
};

export const DEFAULT_PARAMS: Record<string, Record<string, unknown>> = {
  "trigger.manual": {},
  "trigger.schedule": {
    every: "daily",
    interval_minutes: 60,
    hour: 9,
    minute: 0,
    weekday: 1,
    tz_offset_minutes: new Date().getTimezoneOffset(),
  },
  "trigger.git": { cwd: "", branch: "" },
  "trigger.file": { path: "", glob: "" },
  "http.request": { method: "GET", url: "", query: {}, headers: {}, body_type: "none", body: null, timeout_secs: 30 },
  "transform.set": { ops: [{ op: "set", path: "field", value: "" }] },
  "flow.if": { combine: "and", conditions: [{ left: "{{ $json.field }}", op: "eq", right: "" }] },
  "flow.merge": { mode: "append" },
  "code.js": { mode: "all_items", code: "// items is the input array\nreturn items;", timeout_secs: 5 },
  "agent.run": { prompt: "", cwd: "", model: "", max_turns: 15, timeout_secs: 600, web_search: false },
  "file.read": { path: "" },
  "file.write": { path: "", content: "", mode: "overwrite" },
  "util.wait": { seconds: 5 },
};

export const CONDITION_OPS = [
  { value: "eq", label: "equals" },
  { value: "ne", label: "not equal" },
  { value: "gt", label: "greater than" },
  { value: "gte", label: "at least" },
  { value: "lt", label: "less than" },
  { value: "lte", label: "at most" },
  { value: "contains", label: "contains" },
  { value: "notcontains", label: "does not contain" },
  { value: "exists", label: "exists" },
  { value: "notexists", label: "does not exist" },
  { value: "matches", label: "matches regex" },
];

export const SET_OPS = [
  { value: "set", label: "Set" },
  { value: "rename", label: "Rename" },
  { value: "remove", label: "Remove" },
  { value: "keep", label: "Keep only" },
];

// ------------------------------------------------------------- factories

let counter = 0;

export function freshNodeId(): string {
  counter += 1;
  return `n-${Date.now().toString(36)}${counter}${Math.random().toString(36).slice(2, 6)}`;
}

export function uniqueName(base: string, taken: Set<string>): string {
  if (!taken.has(base)) return base;
  for (let i = 2; ; i += 1) {
    const candidate = `${base} ${i}`;
    if (!taken.has(candidate)) return candidate;
  }
}

export function makeNode(entry: CatalogEntry, takenNames: Set<string>, position: [number, number]): WfNode {
  return {
    id: freshNodeId(),
    type: entry.type,
    type_version: entry.type_version,
    name: uniqueName(entry.label, takenNames),
    position,
    disabled: false,
    notes: "",
    on_error: "stop",
    retry: null,
    params: structuredClone(DEFAULT_PARAMS[entry.type] ?? {}),
  };
}

export function newWorkflowDoc(name: string): WorkflowDoc {
  return {
    version: 1,
    id: "",
    name,
    enabled: true,
    project_id: null,
    settings: {
      timeout_secs: 600,
      overlap: "skip",
      min_interval_secs: 0,
      keep_runs: 50,
      capture: "sample",
      max_items_per_port: 10000,
    },
    permissions: {
      network: { enabled: false, hosts: [], private_ips: false },
      code: false,
      fs: { read: [], write: [] },
      agent: false,
    },
    nodes: [
      {
        id: freshNodeId(),
        type: "trigger.manual",
        type_version: 1,
        name: "Start",
        position: [120, 220],
        disabled: false,
        notes: "",
        on_error: "stop",
        retry: null,
        params: {},
      },
    ],
    connections: [],
    state: { last_fired_at: null, last_run_id: null, last_status: null, trigger: {} },
    created_at: "",
    updated_at: "",
  };
}

export function runStatusTone(status: WfRunStatus | string | null | undefined): "success" | "warning" | "danger" | "accent" | "muted" {
  switch (status) {
    case "success":
      return "success";
    case "running":
    case "queued":
      return "accent";
    case "error":
    case "timeout":
      return "danger";
    case "cancelled":
      return "warning";
    default:
      return "muted";
  }
}

export function runStatusLabel(status: WfRunStatus | string | null | undefined): string {
  switch (status) {
    case "success":
      return "Passed";
    case "running":
      return "Running";
    case "queued":
      return "Queued";
    case "error":
      return "Failed";
    case "timeout":
      return "Timed out";
    case "cancelled":
      return "Stopped";
    default:
      return "Idle";
  }
}

/** Compact "what triggers this" summary for cards. */
export function workflowTriggerSummary(doc: WorkflowDoc): string {
  const triggers = doc.nodes.filter((n) => n.type.startsWith("trigger."));
  if (triggers.length === 0) return "No trigger";
  const parts = triggers.map((t) => {
    switch (t.type) {
      case "trigger.manual":
        return "On demand";
      case "trigger.schedule": {
        const p = t.params as { every?: string; interval_minutes?: number; hour?: number; minute?: number };
        if (p.every === "interval") return `Every ${p.interval_minutes ?? 60} min`;
        if (p.every === "weekly") return "Weekly";
        return "Daily";
      }
      case "trigger.git":
        return "On commit";
      case "trigger.file":
        return "On file change";
      default:
        return t.type;
    }
  });
  return parts.join(" · ");
}

/** Human timestamp from the store's epoch-seconds strings. */
export function fromEpoch(secs: string | null | undefined): string {
  const n = Number(secs);
  if (!n) return "";
  return new Date(n * 1000).toLocaleString([], { dateStyle: "medium", timeStyle: "short" });
}

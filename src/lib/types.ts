export type Project = {
  id: string;
  name: string;
  path: string;
  created_at: string;
  last_opened_at: string;
};

/// A non-prose segment of an assistant turn (reasoning or a tool call) that
/// streamed live and is now persisted alongside the reply.
export type MessagePart = {
  kind: "thought" | "tool";
  text: string;
};

export type ChatMessage = {
  id: string;
  role: string;
  content: string;
  images: string[];
  /// Video attachments (agent-produced artifacts). Absent in older data.
  videos?: string[];
  parts?: MessagePart[];
  created_at: string;
};

export type Chat = {
  id: string;
  project_id: string;
  title: string;
  created_at: string;
  updated_at: string;
  messages: ChatMessage[];
  grok_session_id?: string | null;
  provider_id?: string | null;
  model_id?: string | null;
};

export type ModelInfo = {
  id: string;
  label: string;
  kind: "hosted" | "custom" | "endpoint";
  note: string | null;
  is_default: boolean;
};

export type Workspace = {
  projects: Project[];
  chats: Chat[];
};

export type ProviderKind = "acp" | "http";

export type Provider = {
  id: string;
  label: string;
  kind: ProviderKind;
  command: string | null;
  args: string[];
  env: [string, string][];
  accent: string;
  model: string | null;
  base_url: string | null;
  builtin: boolean;
};

export type ProviderView = Provider & {
  available: boolean;
  active: boolean;
};

export type ProviderStatus = {
  installed: boolean;
  version: string | null;
  path: string | null;
  available: boolean;
  kind: ProviderKind;
};

export type PermissionRequest = {
  chatId: string;
  requestId: number;
  params: {
    sessionId?: string;
    toolCall?: { title?: string; kind?: string; toolCallId?: string };
    options?: Array<{ optionId: string; name: string; kind: string }>;
  };
};

// ----- Automations (triggered agent orchestration) -----

export type ExecMode = "shadow" | "write";

export type ScheduleTrigger = {
  kind: "schedule";
  every: string; // "interval" | "daily" | "weekly"
  interval_minutes: number;
  hour: number;
  minute: number;
  weekday: number;
  tz_offset_minutes: number;
};

export type Trigger =
  | { kind: "manual" }
  | ScheduleTrigger
  | { kind: "git"; branch: string | null; last_seen_commit: string | null }
  | { kind: "file"; path: string; glob: string | null; snapshot: string | null };

export type Executor = {
  prompt: string;
  mode: ExecMode;
  tools: string[];
  deny: string[];
  rules: string | null;
  effort: string | null;
  model: string | null;
  max_turns: number;
  cwd: string;
  web_search: boolean;
  json_schema: unknown | null;
  timeout_secs: number;
  report_dir: string | null;
};

export type AutomationState = {
  last_fired_at: number | null;
  last_run_id: string | null;
  last_status: string | null;
  last_idempotency_key: string | null;
};

export type Automation = {
  id: string;
  name: string;
  enabled: boolean;
  project_id: string | null;
  trigger: Trigger;
  executor: Executor;
  overlap: "skip" | "replace";
  retry: { launch_failure_only: boolean; backoff_secs: number[] };
  chain_input: { from: string } | null;
  min_interval_secs: number;
  created_at: string;
  updated_at: string;
  state: AutomationState;
};

export type RunStatus =
  | "queued"
  | "running"
  | "success"
  | "error"
  | "cancelled"
  | "timeout"
  | "maxturns"
  | "launchfailed"
  | "unknown";

export type RunRecord = {
  id: string;
  automation_id: string;
  trigger_reason: string;
  attempt: number;
  mode: ExecMode;
  status: RunStatus;
  started_at: string;
  finished_at: string | null;
  exit_code: number | null;
  stop_reason: string | null;
  session_id: string | null;
  structured_output: unknown | null;
  final_text: string | null;
  error: string | null;
  seen: boolean;
  log_file: string;
};

export type RunOutput = {
  automationId: string;
  runId: string;
  type: "thought" | "text";
  text: string;
};

export type RunFinished = {
  automationId: string;
  runId: string;
  status: RunStatus;
  error: string | null;
};
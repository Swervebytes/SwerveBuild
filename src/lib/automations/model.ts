import type { Automation, RunStatus, Trigger } from "$lib/types";
import type { IconName } from "$lib/components/ui/icons";

/** A fresh, safe-by-default automation: manual trigger, read-only shadow mode. */
export function newAutomation(cwd = "", projectId: string | null = null): Automation {
  return {
    id: "",
    name: "",
    enabled: true,
    project_id: projectId,
    trigger: { kind: "manual" },
    executor: {
      prompt: "",
      mode: "shadow",
      tools: ["read_file", "grep", "list_dir"],
      deny: [],
      rules: null,
      effort: null,
      model: null,
      max_turns: 15,
      cwd,
      web_search: false,
      json_schema: null,
      timeout_secs: 600,
      report_dir: null,
    },
    overlap: "skip",
    retry: { launch_failure_only: true, backoff_secs: [30, 120] },
    chain_input: null,
    min_interval_secs: 0,
    created_at: "",
    updated_at: "",
    state: {
      last_fired_at: null,
      last_run_id: null,
      last_status: null,
      last_idempotency_key: null,
    },
  };
}

/** The webview's current UTC offset in minutes (UTC = local + offset). */
export function currentTzOffset(): number {
  return new Date().getTimezoneOffset();
}

export type Recipe = {
  id: string;
  name: string;
  icon: IconName;
  blurb: string;
  build: (cwd: string, projectId: string | null) => Automation;
};

/** One-click read-only starting points. Each opens the editor pre-filled but
 *  unsaved, so the user reviews before committing. */
export const recipes: Recipe[] = [
  {
    id: "summary",
    name: "Project summary",
    icon: "play",
    blurb: "A read-only agent you trigger with Run now. The safest first test.",
    build: (cwd, projectId) => {
      const a = newAutomation(cwd, projectId);
      a.name = "Project summary";
      a.trigger = { kind: "manual" };
      a.executor.prompt =
        "Read the README and skim the project's file structure. In a few sentences, summarize what this project is, how it's organized, and its current state.";
      return a;
    },
  },
  {
    id: "docdrift",
    name: "Doc-drift check",
    icon: "clock",
    blurb: "Weekly read-only sweep for stale docs vs. the real code.",
    build: (cwd, projectId) => {
      const a = newAutomation(cwd, projectId);
      a.name = "Doc-drift check";
      a.trigger = {
        kind: "schedule",
        every: "weekly",
        interval_minutes: 0,
        hour: 4,
        minute: 0,
        weekday: 0,
        tz_offset_minutes: currentTzOffset(),
      };
      a.executor.prompt =
        "Read the README and docs, then read the actual source files they describe. List any stale claims, dead links, or features the docs miss. Reply with exactly SILENT if everything is in sync.";
      return a;
    },
  },
  {
    id: "loose-ends",
    name: "Find loose ends",
    icon: "search",
    blurb: "Search the code for TODO / FIXME / HACK and summarize what's outstanding.",
    build: (cwd, projectId) => {
      const a = newAutomation(cwd, projectId);
      a.name = "Find loose ends";
      a.trigger = { kind: "manual" };
      a.executor.prompt =
        "Search the codebase for TODO, FIXME, and HACK comments. Group them by file and write a short summary of what's still outstanding. Reply with exactly SILENT if there are none.";
      return a;
    },
  },
  {
    id: "review-file",
    name: "Review a file on change",
    icon: "file",
    blurb: "When a file changes, read it and post a read-only review. Never edits.",
    build: (cwd, projectId) => {
      const a = newAutomation(cwd, projectId);
      a.name = "Review changed file";
      a.trigger = { kind: "file", path: cwd, glob: "*.md", snapshot: null };
      a.min_interval_secs = 10;
      a.executor.prompt =
        "A file just changed (see the trigger data). Read it and give a short, constructive review — clarity, correctness, anything missing. Comment only; change nothing.";
      return a;
    },
  },
];

export function triggerIcon(trigger: Trigger): IconName {
  switch (trigger.kind) {
    case "schedule":
      return "clock";
    case "git":
      return "git-branch";
    case "file":
      return "file";
    default:
      return "play";
  }
}

const DOW = ["Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday"];

function hhmm(h: number, m: number): string {
  const hh = ((h + 11) % 12) + 1;
  const ap = h < 12 ? "AM" : "PM";
  return `${hh}:${String(m).padStart(2, "0")} ${ap}`;
}

export function triggerSummary(trigger: Trigger): string {
  switch (trigger.kind) {
    case "manual":
      return "On demand";
    case "schedule":
      if (trigger.every === "interval") {
        const m = trigger.interval_minutes;
        return m % 60 === 0 ? `Every ${m / 60}h` : `Every ${m} min`;
      }
      if (trigger.every === "weekly") {
        return `Every ${DOW[trigger.weekday] ?? "week"} at ${hhmm(trigger.hour, trigger.minute)}`;
      }
      return `Every day at ${hhmm(trigger.hour, trigger.minute)}`;
    case "git":
      return `On commit to ${trigger.branch ?? "current branch"}`;
    case "file":
      return `When ${trigger.glob ?? "files"} change`;
    default:
      return "Trigger";
  }
}

export function statusTone(
  status: RunStatus | null | undefined,
): "success" | "warning" | "danger" | "accent" | "muted" {
  switch (status) {
    case "success":
      return "success";
    case "running":
    case "queued":
      return "accent";
    case "error":
    case "launchfailed":
    case "timeout":
      return "danger";
    case "maxturns":
      return "warning";
    default:
      return "muted";
  }
}

export function statusLabel(status: RunStatus | null | undefined, silent = false): string {
  switch (status) {
    case "success":
      return silent ? "Silent" : "Passed";
    case "running":
      return "Running";
    case "queued":
      return "Queued";
    case "error":
      return "Failed";
    case "launchfailed":
      return "Launch failed";
    case "timeout":
      return "Timed out";
    case "maxturns":
      return "Max turns";
    case "cancelled":
      return "Stopped";
    default:
      return "Idle";
  }
}

/** Map a failed run's signature to a friendly, actionable message. */
export function friendlyError(status: RunStatus, error: string | null): string {
  switch (status) {
    case "launchfailed":
      return "Grok couldn't start — check it's installed and signed in on the Home screen.";
    case "maxturns":
      return "Hit the max-turns limit before finishing. Raise Max turns in Advanced, or narrow the task.";
    case "timeout":
      return "Stopped after the time limit. Increase the run timeout in Advanced, or simplify the prompt.";
    case "error":
      return error ? `The run reported an error: ${error}` : "The run ended with an error.";
    default:
      return error ?? "The run did not complete.";
  }
}

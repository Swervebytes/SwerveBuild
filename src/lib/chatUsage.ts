// Context / token usage for chat chrome (S14).
// Honest: only display numbers the agent reported. Never invent used/size.

/** Session-level context window usage from ACP `usage_update` (or equivalent). */
export type ChatUsage = {
  /** Tokens currently in context (agent-reported). */
  used: number | null;
  /** Context window size in tokens (agent-reported). */
  size: number | null;
  /** Optional cumulative session cost amount. */
  costAmount: number | null;
  /** ISO 4217 currency when costAmount is set. */
  costCurrency: string | null;
  /** Epoch ms of last real report; null if never reported. */
  updatedAt: number | null;
};

export function emptyUsage(): ChatUsage {
  return {
    used: null,
    size: null,
    costAmount: null,
    costCurrency: null,
    updatedAt: null,
  };
}

export function hasKnownUsage(u: ChatUsage | null | undefined): boolean {
  return (
    !!u &&
    u.used != null &&
    u.size != null &&
    Number.isFinite(u.used) &&
    Number.isFinite(u.size) &&
    u.size > 0 &&
    u.used >= 0
  );
}

/**
 * Tokens known but window size not (yet) known — still real data (S34).
 *
 * Several agents report what they consumed without ever stating the window.
 * Showing that count is honest; only the percentage would be an invention,
 * so callers render the number without a bar until a size is learned.
 */
export function hasKnownUsed(u: ChatUsage | null | undefined): boolean {
  return !!u && u.used != null && Number.isFinite(u.used) && u.used >= 0;
}

export function usagePercent(u: ChatUsage): number | null {
  if (!hasKnownUsage(u) || u.size == null || u.used == null) return null;
  return Math.min(100, Math.max(0, (u.used / u.size) * 100));
}

/** Compact token count: 53000 → "53k", 1500 → "1.5k", 999 → "999". */
export function formatTokens(n: number): string {
  if (!Number.isFinite(n)) return "—";
  const abs = Math.abs(n);
  if (abs < 1000) return String(Math.round(n));
  if (abs < 10_000) {
    const k = n / 1000;
    const s = k.toFixed(1).replace(/\.0$/, "");
    return `${s}k`;
  }
  if (abs < 1_000_000) return `${Math.round(n / 1000)}k`;
  const m = n / 1_000_000;
  return `${m.toFixed(1).replace(/\.0$/, "")}M`;
}

function asFiniteNumber(v: unknown): number | null {
  if (typeof v === "number" && Number.isFinite(v)) return v;
  if (typeof v === "string" && v.trim() !== "") {
    const n = Number(v);
    if (Number.isFinite(n)) return n;
  }
  return null;
}

/**
 * Parse ACP `sessionUpdate: "usage_update"` fields (RFD session-usage).
 * Returns null if used+size are not both present (honest: incomplete = unknown).
 */
export function parseUsageUpdate(inner: Record<string, unknown>): ChatUsage | null {
  const used = asFiniteNumber(inner.used);
  const size = asFiniteNumber(inner.size);
  if (used == null || size == null || size <= 0 || used < 0) return null;

  let costAmount: number | null = null;
  let costCurrency: string | null = null;
  const cost = inner.cost;
  if (cost && typeof cost === "object" && !Array.isArray(cost)) {
    const c = cost as Record<string, unknown>;
    costAmount = asFiniteNumber(c.amount);
    if (typeof c.currency === "string" && c.currency.trim()) {
      costCurrency = c.currency.trim().toUpperCase();
    }
    if (costAmount == null) costCurrency = null;
  }

  return {
    used,
    size,
    costAmount,
    costCurrency,
    updatedAt: Date.now(),
  };
}

/**
 * Parse optional end-turn `usage` on PromptResponse (draft RFD — unstable).
 * Only maps to context bar when both a used total and a window size exist.
 * totalTokens alone is NOT enough (we never invent size).
 */
export function parseEndTurnUsage(usage: unknown): ChatUsage | null {
  if (!usage || typeof usage !== "object" || Array.isArray(usage)) return null;
  const u = usage as Record<string, unknown>;

  // Prefer explicit used/size if an agent reuses the session shape on the result.
  const direct = parseUsageUpdate(u);
  if (direct) return direct;

  // Some agents may nest context under context / contextWindow.
  for (const key of ["context", "contextWindow", "context_window"]) {
    const nested = u[key];
    if (nested && typeof nested === "object" && !Array.isArray(nested)) {
      const parsed = parseUsageUpdate(nested as Record<string, unknown>);
      if (parsed) return parsed;
    }
  }

  return null;
}

/**
 * Vendor-namespaced session updates that carry real token numbers (S34).
 *
 * Grok sends these on `_x.ai/session/update` (not the ACP-standard method —
 * see `acp.rs`, which forwards any vendor-namespaced `session/update`):
 *
 *  - `turn_completed`       → `usage.inputTokens` — context sent, EVERY turn
 *  - `auto_compact_started` → `tokens_used` + `context_window` — both numbers,
 *                             but only when compaction fires (~80% full)
 *
 * May return usage with a null `size`; the caller keeps any window size it has
 * already learned for the session. Nothing here is ever estimated.
 */
export function parseVendorUsage(inner: Record<string, unknown>): ChatUsage | null {
  const kind = typeof inner.sessionUpdate === "string" ? inner.sessionUpdate : "";

  if (kind === "auto_compact_started") {
    const used = asFiniteNumber(inner.tokens_used);
    const size = asFiniteNumber(inner.context_window);
    if (used == null || used < 0 || size == null || size <= 0) return null;
    return {
      used,
      size,
      costAmount: null,
      costCurrency: null,
      updatedAt: Date.now(),
    };
  }

  // `turn_completed` is camelCase; `response_completed` arrives slightly
  // earlier in snake_case. Both are real — accept either (S34b).
  if (kind === "turn_completed" || kind === "response_completed") {
    const raw = inner.usage;
    if (!raw || typeof raw !== "object" || Array.isArray(raw)) return null;
    const u = raw as Record<string, unknown>;
    // inputTokens is what was actually sent as context this turn. totalTokens
    // adds the completion, which is not context-resident — prefer inputTokens.
    const used =
      asFiniteNumber(u.inputTokens) ??
      asFiniteNumber(u.input_tokens) ??
      asFiniteNumber(u.totalTokens);
    if (used == null || used < 0) return null;
    const size = asFiniteNumber(u.contextWindow) ?? asFiniteNumber(u.context_window);
    return {
      used,
      size: size != null && size > 0 ? size : null,
      costAmount: null,
      costCurrency: null,
      updatedAt: Date.now(),
    };
  }

  return null;
}

/**
 * Merge a fresh (possibly partial) report over what we already know.
 *
 * A context window does not change mid-session, so a previously reported
 * `size` is kept when a later update omits it — that is remembering a real
 * number, not inventing one. `used` always comes from the newest report.
 */
export function mergeUsage(prev: ChatUsage, next: ChatUsage): ChatUsage {
  return {
    used: next.used ?? prev.used,
    size: next.size ?? prev.size,
    costAmount: next.costAmount ?? prev.costAmount,
    costCurrency: next.costCurrency ?? prev.costCurrency,
    updatedAt: next.updatedAt ?? prev.updatedAt,
  };
}

export function usageTooltip(u: ChatUsage | null | undefined): string {
  if (!hasKnownUsage(u) || !u) {
    // Tokens known, window not: say so plainly rather than showing nothing.
    if (hasKnownUsed(u) && u) {
      return `Context: ${u.used!.toLocaleString()} tokens used. This agent has not reported its context window size, so no percentage is shown (never estimated).`;
    }
    return "Context usage not reported by this agent. Appears when the ACP agent sends usage_update (used / size tokens). Never estimated.";
  }
  const used = u.used!;
  const size = u.size!;
  const pct = usagePercent(u);
  const pctText = pct != null ? ` (${Math.round(pct)}%)` : "";
  let tip = `Context: ${used.toLocaleString()} of ${size.toLocaleString()} tokens${pctText}`;
  if (u.costAmount != null && u.costCurrency) {
    tip += ` · Session cost ≈ ${u.costAmount} ${u.costCurrency}`;
  }
  return tip;
}

/** Tone for high utilization (ACP RFD client guidance). */
export function usageTone(u: ChatUsage | null | undefined): "ok" | "warn" | "high" | "critical" | "unknown" {
  const pct = u ? usagePercent(u) : null;
  if (pct == null) return "unknown";
  if (pct > 95) return "critical";
  if (pct >= 90) return "high";
  if (pct >= 75) return "warn";
  return "ok";
}

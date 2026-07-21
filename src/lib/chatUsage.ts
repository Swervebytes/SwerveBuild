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

export function usageTooltip(u: ChatUsage | null | undefined): string {
  if (!hasKnownUsage(u) || !u) {
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

import { describe, expect, it } from "vitest";
import {
  emptyUsage,
  formatTokens,
  hasKnownUsage,
  hasKnownUsed,
  mergeUsage,
  parseEndTurnUsage,
  parseUsageUpdate,
  parseVendorUsage,
  sizeOnlyUsage,
  usagePercent,
  usageTone,
} from "./chatUsage";

/**
 * Fixtures below are REAL payloads captured off the ACP wire from
 * `grok agent stdio` (S34b/S35), not hand-written guesses.
 *
 * This distinction is the reason these tests exist: S34 shipped a parser that
 * passed against payloads copied from Grok's own on-disk log, where the events
 * are recorded under `session/update`. On the wire they actually arrive as
 * `_x.ai/session_notification`, so the feature was dead on delivery and the
 * context bar stayed at "—". Keep these fixtures wire-accurate.
 */

/** `_x.ai/session_notification` → fires on every completed turn (camelCase). */
const TURN_COMPLETED = {
  sessionUpdate: "turn_completed",
  prompt_id: "0ecfd2af-2542-4c42-b73d-49fefaf14664",
  stop_reason: "end_turn",
  usage: {
    inputTokens: 14186,
    outputTokens: 29,
    totalTokens: 14215,
    cachedReadTokens: 2560,
    cacheCreationTokens: 0,
    reasoningTokens: 24,
    modelCalls: 1,
    apiDurationMs: 1593,
    costUsdTicks: 241940000,
    modelUsage: {
      // NOTE: this id appears in no model catalog — do not try to look it up.
      "grok-4.5-build": { inputTokens: 14186, outputTokens: 29, totalTokens: 14215 },
    },
    numTurns: 1,
  },
};

/** Same notification stream, slightly earlier, snake_case. */
const RESPONSE_COMPLETED = {
  sessionUpdate: "response_completed",
  usage: {
    input_tokens: 11626,
    output_tokens: 29,
    cache_read_input_tokens: 2560,
    cache_creation_input_tokens: 0,
    reasoning_tokens: 24,
  },
};

/** Only fires once the context is ~80% full — but carries BOTH numbers. */
const AUTO_COMPACT = {
  sessionUpdate: "auto_compact_started",
  tokens_used: 181312,
  context_window: 200000,
  percentage: 91,
  reason: "Context window 91% full",
};

describe("parseVendorUsage — Grok session notifications", () => {
  it("reads context sent from turn_completed", () => {
    const u = parseVendorUsage(TURN_COMPLETED)!;
    expect(u).not.toBeNull();
    // inputTokens, not totalTokens: the completion is not context-resident.
    expect(u.used).toBe(14186);
    expect(u.size).toBeNull();
    expect(hasKnownUsed(u)).toBe(true);
    expect(hasKnownUsage(u)).toBe(false);
    // Without a window we must not produce a percentage.
    expect(usagePercent(u)).toBeNull();
  });

  it("reads snake_case response_completed", () => {
    const u = parseVendorUsage(RESPONSE_COMPLETED)!;
    expect(u.used).toBe(11626);
    expect(u.size).toBeNull();
  });

  it("reads both numbers from auto_compact_started", () => {
    const u = parseVendorUsage(AUTO_COMPACT)!;
    expect(u.used).toBe(181312);
    expect(u.size).toBe(200000);
    expect(hasKnownUsage(u)).toBe(true);
    // Cross-check against the percentage Grok computed itself.
    expect(Math.round(usagePercent(u)!)).toBe(AUTO_COMPACT.percentage);
  });

  it("ignores updates that carry no usage", () => {
    expect(parseVendorUsage({ sessionUpdate: "tool_call" })).toBeNull();
    expect(parseVendorUsage({ sessionUpdate: "agent_message_chunk" })).toBeNull();
    expect(parseVendorUsage({ sessionUpdate: "turn_completed" })).toBeNull();
    expect(parseVendorUsage({})).toBeNull();
  });

  it("never invents a number from a partial payload", () => {
    // Compaction notice missing the window: unusable, not half-usable.
    expect(parseVendorUsage({ sessionUpdate: "auto_compact_started", tokens_used: 5 })).toBeNull();
    expect(
      parseVendorUsage({ sessionUpdate: "auto_compact_started", context_window: 200000 }),
    ).toBeNull();
    // Non-numeric junk must not slip through.
    expect(
      parseVendorUsage({ sessionUpdate: "turn_completed", usage: { inputTokens: "lots" } }),
    ).toBeNull();
  });
});

describe("sizeOnlyUsage — window learned at session start (S35)", () => {
  it("accepts a real window with no tokens yet", () => {
    const u = sizeOnlyUsage(500000)!;
    expect(u.size).toBe(500000);
    expect(u.used).toBeNull();
    // A window alone is not usage — nothing to display yet.
    expect(hasKnownUsed(u)).toBe(false);
    expect(hasKnownUsage(u)).toBe(false);
  });

  it("rejects missing or nonsense windows", () => {
    expect(sizeOnlyUsage(null)).toBeNull();
    expect(sizeOnlyUsage(undefined)).toBeNull();
    expect(sizeOnlyUsage(0)).toBeNull();
    expect(sizeOnlyUsage(-1)).toBeNull();
  });
});

describe("mergeUsage — remembering vs inventing", () => {
  it("gives a true percentage on the first turn (S35 end-to-end)", () => {
    // session/new advertises the window, then the first turn reports tokens.
    let u = mergeUsage(emptyUsage(), sizeOnlyUsage(500000)!);
    u = mergeUsage(u, parseVendorUsage(TURN_COMPLETED)!);
    expect(hasKnownUsage(u)).toBe(true);
    expect(u.used).toBe(14186);
    expect(u.size).toBe(500000);
    expect(Math.round(usagePercent(u)!)).toBe(3);
  });

  it("keeps a known window when a later report omits it", () => {
    // A context window does not change mid-session, so this is memory.
    let u = mergeUsage(emptyUsage(), parseVendorUsage(AUTO_COMPACT)!);
    u = mergeUsage(u, parseVendorUsage(TURN_COMPLETED)!);
    expect(u.size).toBe(200000);
    expect(u.used).toBe(14186);
  });

  it("advances used to the newest report", () => {
    const first = mergeUsage(emptyUsage(), parseVendorUsage(RESPONSE_COMPLETED)!);
    const second = mergeUsage(first, parseVendorUsage(TURN_COMPLETED)!);
    expect(second.used).toBe(14186);
  });
});

describe("strict ACP shapes still require both numbers", () => {
  it("usage_update needs used and size", () => {
    expect(parseUsageUpdate({ used: 10 })).toBeNull();
    expect(parseUsageUpdate({ size: 100 })).toBeNull();
    expect(parseUsageUpdate({ used: 10, size: 0 })).toBeNull();
    expect(parseUsageUpdate({ used: 10, size: 100 })?.used).toBe(10);
  });

  it("end-turn totals alone are not a context reading", () => {
    // totalTokens without a window would only be a percentage if we guessed.
    expect(parseEndTurnUsage({ totalTokens: 100 })).toBeNull();
    expect(parseEndTurnUsage(null)).toBeNull();
    expect(parseEndTurnUsage({ used: 5, size: 50 })?.size).toBe(50);
  });
});

describe("display helpers", () => {
  it("formats token counts compactly", () => {
    expect(formatTokens(999)).toBe("999");
    expect(formatTokens(1500)).toBe("1.5k");
    expect(formatTokens(14186)).toBe("14k");
    expect(formatTokens(500000)).toBe("500k");
    expect(formatTokens(1_200_000)).toBe("1.2M");
  });

  it("escalates tone as the window fills", () => {
    const at = (used: number, size: number) =>
      usageTone({ used, size, costAmount: null, costCurrency: null, updatedAt: 1 });
    expect(at(10, 100)).toBe("ok");
    expect(at(80, 100)).toBe("warn");
    expect(at(92, 100)).toBe("high");
    expect(at(99, 100)).toBe("critical");
    expect(usageTone(emptyUsage())).toBe("unknown");
  });
});

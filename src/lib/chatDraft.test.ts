import { describe, expect, it } from "vitest";
import { fitDraft, shouldRecoverDraft, type ChatDraft } from "./chatDraft";

function draft(over: Partial<ChatDraft> = {}): ChatDraft {
  return {
    chatId: "c1",
    content: "The reply.",
    parts: [{ kind: "tool", text: "ran a tool" }],
    images: [],
    videos: [],
    updatedAt: 2_000,
    ...over,
  };
}

describe("shouldRecoverDraft", () => {
  it("restores a reply that died after the user's question", () => {
    expect(
      shouldRecoverDraft(draft(), [
        { role: "user", content: "question", created_at: "1970-01-01T00:00:01.000Z" },
      ]),
    ).toBe(true);
  });

  it("does not restore a draft from before the latest question", () => {
    expect(
      shouldRecoverDraft(draft({ updatedAt: 500 }), [
        { role: "user", content: "newer question", created_at: "1970-01-01T00:00:05.000Z" },
      ]),
    ).toBe(false);
  });

  it("does not duplicate a reply that already saved", () => {
    expect(
      shouldRecoverDraft(draft(), [
        { role: "user", content: "question", created_at: "1970-01-01T00:00:01.000Z" },
        { role: "assistant", content: "The reply.", created_at: "1970-01-01T00:00:03.000Z" },
      ]),
    ).toBe(false);
  });
});

describe("fitDraft", () => {
  it("drops old tool text before it cuts the reply", () => {
    const huge = "x".repeat(2_000_000);
    const fitted = fitDraft(
      draft({
        content: "keep this reply",
        parts: [
          { kind: "tool", text: huge },
          { kind: "tool", text: huge },
        ],
      }),
    );
    expect(fitted.content).toBe("keep this reply");
    expect(fitted.parts.length).toBeLessThan(2);
  });
});

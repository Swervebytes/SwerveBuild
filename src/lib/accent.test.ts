import { describe, expect, it } from "vitest";
import { accentForeground } from "./accent";

describe("accentForeground (P4.1 contrast guard)", () => {
  it("keeps dark text on the shipped light-ish accents", () => {
    // Builtin provider accents today — all mid/light, dark text stays correct.
    expect(accentForeground("#6cb5ff")).toBe("#04060d"); // grok
    expect(accentForeground("#d97757")).toBe("#04060d"); // claude
    expect(accentForeground("#6c8cff")).toBe("#04060d"); // gemini
  });

  it("flips to white on dark accents — the audit E failure case", () => {
    expect(accentForeground("#1a1a2e")).toBe("#ffffff");
    expect(accentForeground("#04060d")).toBe("#ffffff");
    expect(accentForeground("#7a1010")).toBe("#ffffff");
  });

  it("tolerates missing hash and garbage without throwing", () => {
    expect(accentForeground("6cb5ff")).toBe("#04060d");
    expect(accentForeground("not-a-color")).toBe("#04060d");
    expect(accentForeground("")).toBe("#04060d");
  });
});

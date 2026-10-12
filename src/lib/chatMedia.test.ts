import { describe, expect, it } from "vitest";
import { isChatMediaTool, mediaScanText, rememberToolName } from "./chatMedia";

describe("chat media tools", () => {
  it("accepts the screenshot and capture tools, including the MCP prefix", () => {
    expect(isChatMediaTool("app_ui_screenshot")).toBe(true);
    expect(isChatMediaTool("swervebuild__browser_screenshot")).toBe(true);
    expect(isChatMediaTool("swervebuild_vm_screenshot")).toBe(true);
    expect(isChatMediaTool("media_capture_still")).toBe(true);
    expect(isChatMediaTool("media_encode_clip")).toBe(true);
    expect(isChatMediaTool("local_image_generate")).toBe(true);
  });

  it("rejects file reads, prose, and guest clicks that happen to return a png", () => {
    expect(isChatMediaTool("ReadFile")).toBe(false);
    expect(isChatMediaTool("grep")).toBe(false);
    expect(isChatMediaTool("vm_click")).toBe(false);
    expect(isChatMediaTool("vm_type")).toBe(false);
    expect(isChatMediaTool("vm_press")).toBe(false);
    expect(isChatMediaTool(undefined)).toBe(false);
  });

  it("keeps the tool name when a later update replaces the title", () => {
    const first = rememberToolName({
      title: "app_ui_screenshot",
      _meta: { "x.ai/tool": { name: "swervebuild__app_ui_screenshot" } },
    });
    const later = rememberToolName(
      { title: "docs/hero.svg is mentioned in the notes" },
      first,
    );
    expect(later).toBe("swervebuild__app_ui_screenshot");
    expect(isChatMediaTool(later)).toBe(true);
  });

  it("does not show an image path that was only mentioned", () => {
    const text = mediaScanText([
      {
        role: "assistant",
        content: "The file docs/hero.svg is the README banner.",
      },
      {
        role: "tool",
        toolName: "ReadFile",
        content: "see docs/hero.svg in GO-PUBLIC.md",
      },
      {
        role: "tool",
        toolName: "vm_click",
        content: String.raw`{"path":"C:\\shots\\after-click.png"}`,
      },
    ]);
    expect(text).toBe("");
  });

  it("uses the MCP name inside a use_tool wrapper", () => {
    const first = rememberToolName({
      title: "use_tool",
      rawInput: { tool_name: "swervebuild__app_ui_screenshot", tool_input: {} },
      _meta: { "x.ai/tool": { name: "use_tool" } },
    });
    expect(first).toBe("swervebuild__app_ui_screenshot");
    const done = rememberToolName(
      {
        title: "swervebuild__app_ui_screenshot",
        rawOutput: { type: "MCP", tool_name: "app_ui_screenshot" },
        _meta: { "x.ai/tool": { name: "use_tool" } },
      },
      first,
    );
    expect(isChatMediaTool(done)).toBe(true);
  });

  it("does not treat a search titled app_ui_screenshot as a screenshot", () => {
    const name = rememberToolName({
      title: "app_ui_screenshot",
      _meta: { "x.ai/tool": { name: "grep" } },
    });
    expect(name).toBe("grep");
    expect(isChatMediaTool(name)).toBe(false);
  });

  it("shows the path a screenshot tool returned", () => {
    const shot = String.raw`{"path":"C:\\Users\\dev\\.swervebuild\\app_ui_artifacts\\shot.png"}`;
    const text = mediaScanText([
      { role: "assistant", content: "Here is docs/hero.svg" },
      { role: "tool", toolName: "ReadFile", content: "docs/hero.svg" },
      { role: "tool", toolName: "swervebuild__app_ui_screenshot", content: shot },
    ]);
    expect(text).toBe(shot);
  });
});

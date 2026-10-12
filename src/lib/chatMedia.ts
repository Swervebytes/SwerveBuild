/** Tools that return a picture or clip for this chat to show.
 *  A path mentioned in prose, a file read, or another tool stays text. */
const CHAT_MEDIA_TOOLS = new Set([
  "app_ui_screenshot",
  "browser_screenshot",
  "vm_screenshot",
  "media_capture_still",
  "media_encode_clip",
  "local_image_generate",
]);

export function normalizeToolName(raw: string): string {
  return raw
    .trim()
    .toLowerCase()
    .replace(/^swervebuild__/, "")
    .replace(/^swervebuild_/, "");
}

export function isChatMediaTool(name: string | null | undefined): boolean {
  if (!name) return false;
  return CHAT_MEDIA_TOOLS.has(normalizeToolName(name));
}

function metaToolName(inner: Record<string, unknown>): string | null {
  const meta = inner._meta;
  if (!meta || typeof meta !== "object") return null;
  const tool = (meta as Record<string, unknown>)["x.ai/tool"];
  if (!tool || typeof tool !== "object") return null;
  const name = (tool as Record<string, unknown>).name;
  return typeof name === "string" && name.trim() ? name.trim() : null;
}

function stringField(node: unknown, key: string): string | null {
  if (!node || typeof node !== "object") return null;
  const value = (node as Record<string, unknown>)[key];
  return typeof value === "string" && value.trim() ? value.trim() : null;
}

/** MCP calls arrive as `use_tool`. The real name is inside the payload. */
function wrappedToolName(inner: Record<string, unknown>): string | null {
  return (
    stringField(inner.rawInput, "tool_name") ??
    stringField(inner.rawOutput, "tool_name") ??
    null
  );
}

/** Keep the real tool name. A later update replaces `title` with a summary. */
export function rememberToolName(
  inner: Record<string, unknown>,
  previous?: string,
): string | undefined {
  const fromMeta = metaToolName(inner);
  const wrapped = !fromMeta || normalizeToolName(fromMeta) === "use_tool";
  if (!wrapped && fromMeta) return fromMeta;
  const innerName = wrappedToolName(inner);
  if (innerName) return innerName;
  if (previous && normalizeToolName(previous) !== "use_tool") return previous;
  if (fromMeta) return fromMeta;
  const title = typeof inner.title === "string" ? inner.title.trim() : "";
  if (title && /^[A-Za-z0-9_.:-]{1,80}$/.test(title)) return title;
  return undefined;
}

/** Text whose image paths may be shown. Prose and other tools contribute nothing. */
export function mediaScanText(
  items: { role: string; toolName?: string; content: string }[],
): string {
  return items
    .filter((item) => item.role === "tool" && isChatMediaTool(item.toolName))
    .map((item) => item.content)
    .filter(Boolean)
    .join("\n");
}

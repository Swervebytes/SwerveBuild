import type { MessagePart } from "$lib/types";

/** A reply that was streaming when the app died, before the turn could save. */
export type ChatDraft = {
  chatId: string;
  content: string;
  parts: MessagePart[];
  images: string[];
  videos: string[];
  updatedAt: number;
};

const PREFIX = "swerve-chat-draft-v1:";
const MAX_CHARS = 3_500_000;

export function saveChatDraft(draft: ChatDraft): void {
  try {
    const fitted = fitDraft(draft);
    localStorage.setItem(PREFIX + draft.chatId, JSON.stringify(fitted));
  } catch {
    /* A full disk or a private window must not break the chat. */
  }
}

export function loadChatDraft(chatId: string): ChatDraft | null {
  try {
    const raw = localStorage.getItem(PREFIX + chatId);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as ChatDraft;
    if (!parsed || parsed.chatId !== chatId || typeof parsed.updatedAt !== "number") return null;
    if (typeof parsed.content !== "string") return null;
    return parsed;
  } catch {
    return null;
  }
}

export function clearChatDraft(chatId: string): void {
  try {
    localStorage.removeItem(PREFIX + chatId);
  } catch {
    /* ignore */
  }
}

/** Keep the written reply. Drop the oldest tool text if the draft is huge. */
export function fitDraft(draft: ChatDraft): ChatDraft {
  let parts = [...(draft.parts ?? [])];
  let content = draft.content ?? "";
  const pack = () =>
    JSON.stringify({ ...draft, content, parts });
  while (pack().length > MAX_CHARS && parts.length > 0) {
    parts = parts.slice(1);
  }
  if (pack().length > MAX_CHARS) {
    content = content.slice(-200_000);
  }
  return { ...draft, content, parts };
}

/**
 * Recover only a draft written after the latest saved user message, and only
 * when that reply was never saved.
 */
export function shouldRecoverDraft(
  draft: ChatDraft,
  messages: { role: string; content: string; created_at: string }[],
): boolean {
  const text = draft.content.trim();
  const hasParts = (draft.parts ?? []).some((part) => part.text.trim().length > 0);
  if (!text && !hasParts && draft.images.length === 0 && draft.videos.length === 0) {
    return false;
  }
  const last = messages[messages.length - 1];
  if (!last) return true;
  if (last.role === "assistant" && last.content.trim() === text) return false;
  if (last.role !== "user") return false;
  const userAt = Date.parse(last.created_at);
  if (Number.isFinite(userAt) && draft.updatedAt + 2000 < userAt) return false;
  return true;
}

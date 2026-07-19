// Global tool-approval queue.
//
// A `session/request_permission` can fire for ANY chat with a live session —
// including a background chat while the user is on Projects, Settings, or the
// Home page. Previously the only listener lived on the chat route, so an
// approval needed off-route silently hung the background run (and undercut the
// "up to 3 concurrent sessions" promise). This store owns one app-lifetime
// listener and renders through the root layout, so approvals always surface.

import { listen } from "@tauri-apps/api/event";
import { invoke } from "@tauri-apps/api/core";
import type { PermissionRequest } from "$lib/types";
import { workspaceStore } from "$lib/stores/workspace.svelte";

type QueuedPermission = PermissionRequest & { chatTitle: string };

let queue = $state<QueuedPermission[]>([]);
let started = false;

async function enqueue(request: PermissionRequest) {
  const isDup = () =>
    queue.some((q) => q.chatId === request.chatId && q.requestId === request.requestId);
  if (isDup()) return;

  // Resolve the chat title from the already-loaded workspace (no round-trip);
  // only refresh if we miss (a brand-new chat may not be cached yet).
  let title = workspaceStore.chats.find((c) => c.id === request.chatId)?.title;
  if (!title) {
    await workspaceStore.refresh();
    title = workspaceStore.chats.find((c) => c.id === request.chatId)?.title;
  }
  if (isDup()) return; // re-check: an identical event may have landed during the await
  queue = [...queue, { ...request, chatTitle: title ?? "Chat" }];
}

export const permissionStore = {
  get current(): QueuedPermission | null {
    return queue[0] ?? null;
  },
  get queueLength() {
    return queue.length;
  },
  /// Begin listening. Idempotent — safe to call from the layout's onMount.
  async start() {
    if (started) return;
    started = true;
    await listen<PermissionRequest>("chat-permission-request", (event) => {
      void enqueue(event.payload);
    });
  },
  async respond(optionId: string) {
    const pending = queue[0];
    if (!pending) return;
    try {
      await invoke("respond_chat_permission", {
        chatId: pending.chatId,
        requestId: pending.requestId,
        optionId,
      });
    } finally {
      // Drop it whether or not the backend accepted: a failed respond leaves a
      // dead request, and keeping it would wedge the queue on the same modal.
      queue = queue.filter((q) => q !== pending);
    }
  },
};

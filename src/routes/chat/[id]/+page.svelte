<script lang="ts">
  import { onMount } from "svelte";
  import { page } from "$app/stores";
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { goto } from "$app/navigation";
  import type { Chat, ChatMessage, PermissionRequest, Project } from "$lib/types";
  import { loadWorkspace, projectById } from "$lib/workspace";
  import { workspaceStore } from "$lib/stores/workspace.svelte";
  import { imageSrc } from "$lib/attachments";
  import ChatHeader from "$lib/components/chat/ChatHeader.svelte";
  import MessageList from "$lib/components/chat/MessageList.svelte";
  import Composer from "$lib/components/chat/Composer.svelte";
  import PermissionModal from "$lib/components/chat/PermissionModal.svelte";

  type StreamMessage = {
    id: string;
    role: "assistant" | "tool";
    content: string;
    kind?: string;
  };

  let chat = $state<Chat | null>(null);
  let project = $state<Project | null>(null);
  let streaming = $state<StreamMessage[]>([]);
  let sessionReady = $state(false);
  let sending = $state(false);
  let error = $state<string | null>(null);
  let activeSessionCount = $state(0);
  let permissionQueue = $state<Array<PermissionRequest & { chatTitle: string }>>([]);

  const chatId = $derived($page.params.id);
  const currentPermission = $derived(permissionQueue[0] ?? null);

  let unlistenUpdate: (() => void) | null = null;
  let unlistenReady: (() => void) | null = null;
  let unlistenPermission: (() => void) | null = null;
  let bootstrapGen = 0;

  function resetStream() {
    streaming = [];
  }

  function appendStream(update: Record<string, unknown>) {
    const params = (update.params as Record<string, unknown>) ?? update;
    const inner = (params.update as Record<string, unknown>) ?? params;
    const sessionUpdate = String(inner.sessionUpdate ?? "");
    const text =
      ((inner.content as Record<string, unknown>)?.text as string) ??
      (inner.title as string) ??
      "";

    if (!text && sessionUpdate !== "tool_call") return;

    if (sessionUpdate === "agent_message_chunk") {
      const last = streaming[streaming.length - 1];
      if (last?.role === "assistant" && last.kind === "message") {
        last.content += text;
        streaming = [...streaming];
      } else {
        streaming = [
          ...streaming,
          { id: crypto.randomUUID(), role: "assistant", content: text, kind: "message" },
        ];
      }
      return;
    }

    if (sessionUpdate === "agent_thought_chunk") {
      const last = streaming[streaming.length - 1];
      if (last?.role === "assistant" && last.kind === "thought") {
        last.content += text;
        streaming = [...streaming];
      } else {
        streaming = [
          ...streaming,
          { id: crypto.randomUUID(), role: "assistant", content: text, kind: "thought" },
        ];
      }
      return;
    }

    if (sessionUpdate === "tool_call" || sessionUpdate === "tool_call_update") {
      streaming = [
        ...streaming,
        {
          id: crypto.randomUUID(),
          role: "tool",
          content: text || sessionUpdate,
          kind: sessionUpdate,
        },
      ];
    }
  }

  async function finalizeAssistantMessage() {
    const text = streaming
      .filter((item) => item.role === "assistant" && item.kind === "message")
      .map((item) => item.content)
      .join("")
      .trim();

    if (!text || !chat) return;

    const saved = await invoke<ChatMessage>("append_chat_message", {
      chatId: chat.id,
      role: "assistant",
      content: text,
      images: [],
    });

    chat = {
      ...chat,
      messages: [...chat.messages, saved],
      updated_at: saved.created_at,
    };
    resetStream();
  }

  async function refreshActiveSessions() {
    activeSessionCount = (await invoke<string[]>("list_active_chat_sessions")).length;
    workspaceStore.refreshSessions();
  }

  async function bootstrap(id: string, gen: number) {
    error = null;
    sessionReady = false;
    resetStream();

    const workspace = await loadWorkspace();
    const loaded = workspace.chats.find((item) => item.id === id);
    if (!loaded) {
      await goto("/projects");
      return;
    }

    if (gen !== bootstrapGen) return;

    chat = loaded;
    project = projectById(workspace, loaded.project_id) ?? null;

    const result = await invoke<{ success: boolean; message: string }>("start_chat_session", {
      chatId: id,
    });

    if (gen !== bootstrapGen) return;

    if (!result.success) {
      error = result.message;
      return;
    }

    sessionReady = true;
    await refreshActiveSessions();
  }

  async function enqueuePermission(request: PermissionRequest) {
    const workspace = await loadWorkspace();
    const title = workspace.chats.find((item) => item.id === request.chatId)?.title ?? "Chat";
    if (
      permissionQueue.some(
        (item) => item.chatId === request.chatId && item.requestId === request.requestId,
      )
    ) {
      return;
    }
    permissionQueue = [...permissionQueue, { ...request, chatTitle: title }];
  }

  async function respondPermission(optionId: string) {
    const pending = permissionQueue[0];
    if (!pending) return;

    try {
      await invoke("respond_chat_permission", {
        chatId: pending.chatId,
        requestId: pending.requestId,
        optionId,
      });
      permissionQueue = permissionQueue.slice(1);
    } catch (err) {
      error = String(err);
    }
  }

  $effect(() => {
    const id = $page.params.id;
    if (!id) return;
    bootstrapGen += 1;
    const gen = bootstrapGen;
    bootstrap(id, gen);
  });

  async function sendMessage(text: string, images: string[]) {
    if (!chat || sending) return;
    if (!text && images.length === 0) return;

    sending = true;
    error = null;
    resetStream();

    try {
      const userMessage = await invoke<ChatMessage>("append_chat_message", {
        chatId: chat.id,
        role: "user",
        content: text,
        images,
      });

      chat = {
        ...chat,
        title: chat.title === "New chat" && text ? text.slice(0, 48) : chat.title,
        messages: [...chat.messages, userMessage],
        updated_at: userMessage.created_at,
      };

      await invoke("send_chat_message", {
        chatId: chat.id,
        text,
        images,
      });

      await finalizeAssistantMessage();
      workspaceStore.refresh();
    } catch (err) {
      error = String(err);
    } finally {
      sending = false;
    }
  }

  onMount(() => {
    listen<{ chatId: string; params: Record<string, unknown> }>("chat-update", (event) => {
      if (event.payload.chatId !== $page.params.id) return;
      appendStream(event.payload.params);
    }).then((unlisten) => {
      unlistenUpdate = unlisten;
    });

    listen<{ chatId: string }>("chat-session-ready", (event) => {
      if (event.payload.chatId === $page.params.id) {
        sessionReady = true;
      }
      refreshActiveSessions();
    }).then((unlisten) => {
      unlistenReady = unlisten;
    });

    listen<PermissionRequest>("chat-permission-request", (event) => {
      enqueuePermission(event.payload);
    }).then((unlisten) => {
      unlistenPermission = unlisten;
    });

    return () => {
      unlistenUpdate?.();
      unlistenReady?.();
      unlistenPermission?.();
    };
  });
</script>

<div class="chat-page">
  <div class="reading-col rail-wrap">
    <ChatHeader
      title={chat?.title ?? "Chat"}
      projectName={project?.name}
      projectPath={project?.path}
      connected={sessionReady}
      {activeSessionCount}
    />
  </div>

  {#if chat}
    <MessageList messages={chat.messages} {streaming} {imageSrc} />
  {:else}
    <div class="loading reading-col">Loading…</div>
  {/if}

  {#if error}
    <div class="reading-col">
      <p class="error">{error}</p>
    </div>
  {/if}

  <div class="reading-col composer-wrap">
    <Composer disabled={!sessionReady} {sending} onsend={sendMessage} />
  </div>
</div>

{#if currentPermission}
  <PermissionModal
    request={currentPermission}
    queueLength={permissionQueue.length}
    isBackground={currentPermission.chatId !== chatId}
    onrespond={respondPermission}
  />
{/if}

<style>
  .chat-page {
    display: flex;
    flex-direction: column;
    height: calc(100vh - var(--titlebar-height) - 3rem);
    margin: -1.5rem -2rem;
    padding: 1.25rem 2rem 0;
  }

  .rail-wrap {
    flex-shrink: 0;
  }

  .loading {
    flex: 1;
    display: grid;
    place-items: center;
    color: var(--text-muted);
    font-size: 0.875rem;
  }

  .composer-wrap {
    padding-bottom: 1.25rem;
  }

  .error {
    color: var(--danger);
    font-size: 0.8125rem;
    padding: 0.5rem 0.75rem;
    background: var(--danger-tint);
    border-radius: var(--radius-sm);
    margin-bottom: 0.75rem;
  }
</style>

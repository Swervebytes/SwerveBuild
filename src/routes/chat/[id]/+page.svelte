<script lang="ts">
  import { onMount } from "svelte";
  import { page } from "$app/stores";
  import { invoke } from "@tauri-apps/api/core";
  import { listen } from "@tauri-apps/api/event";
  import { goto } from "$app/navigation";
  import type { Chat, ChatMessage, Project } from "$lib/types";
  import { workspaceStore } from "$lib/stores/workspace.svelte";
  import { providerStore } from "$lib/stores/providers.svelte";
  import { imageSrc } from "$lib/attachments";
  import ChatHeader from "$lib/components/chat/ChatHeader.svelte";
  import MessageList from "$lib/components/chat/MessageList.svelte";
  import Composer from "$lib/components/chat/Composer.svelte";

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
  let modelSwitching = $state(false);
  let error = $state<string | null>(null);
  let activeSessionCount = $state(0);

  // `-m` is a grok flag; hide the model picker when another agent backs this chat.
  const chatProviderId = $derived(chat?.provider_id ?? providerStore.active?.id ?? "grok");

  let unlistenUpdate: (() => void) | null = null;
  let unlistenReady: (() => void) | null = null;
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

  async function finalizeAssistantMessage(note?: string) {
    let text = streaming
      .filter((item) => item.role === "assistant" && item.kind === "message")
      .map((item) => item.content)
      .join("")
      .trim();

    if (!text) return false;
    if (!chat) return false;

    if (note) {
      text = `${text}\n\n_${note}_`;
    }

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
    return true;
  }

  async function refreshActiveSessions() {
    activeSessionCount = (await invoke<string[]>("list_active_chat_sessions")).length;
    workspaceStore.refreshSessions();
  }

  async function bootstrap(id: string, gen: number) {
    error = null;
    sessionReady = false;
    resetStream();

    await workspaceStore.refresh();

    let loaded = workspaceStore.chats.find((item) => item.id === id);
    if (!loaded) {
      try {
        loaded = await invoke<Chat>("get_chat", { chatId: id });
      } catch {
        loaded = undefined;
      }
    }

    if (!loaded) {
      await goto("/projects");
      return;
    }

    if (gen !== bootstrapGen) return;

    chat = loaded;
    project = workspaceStore.projectById(loaded.project_id) ?? null;

    try {
      await invoke<{ success: boolean; message: string }>("start_chat_session", {
        chatId: id,
      });
      if (gen !== bootstrapGen) return;
      sessionReady = true;
      await refreshActiveSessions();
    } catch (err) {
      if (gen !== bootstrapGen) return;
      sessionReady = false;
      error =
        "Live Grok session disconnected. Your saved messages are below — send a message to reconnect.";
      if (String(err).trim()) {
        error = `${error} (${String(err)})`;
      }
    }
  }

  $effect(() => {
    const id = $page.params.id;
    if (!id) return;
    bootstrapGen += 1;
    const gen = bootstrapGen;
    bootstrap(id, gen);
  });

  /// Mid-chat model switch: persist the choice, respawn the agent with the new
  /// `-m`, and let `session/load` restore the conversation — feels in-place.
  async function switchModel(id: string | null) {
    if (!chat || modelSwitching || sending) return;
    modelSwitching = true;
    error = null;
    try {
      await invoke("set_chat_model", { chatId: chat.id, modelId: id });
      chat = { ...chat, model_id: id };
      await invoke("close_chat_session", { chatId: chat.id });
      sessionReady = false;
      await invoke("start_chat_session", { chatId: chat.id });
      sessionReady = true;
      await refreshActiveSessions();
    } catch (err) {
      sessionReady = false;
      error = `Model switch failed: ${String(err)} — send a message to reconnect.`;
    } finally {
      modelSwitching = false;
    }
  }

  async function sendMessage(text: string, images: string[]) {
    if (!chat || sending) return;
    if (!text && images.length === 0) return;

    sending = true;
    error = null;
    resetStream();

    try {
      if (!sessionReady) {
        await invoke("start_chat_session", { chatId: chat.id });
        sessionReady = true;
        await refreshActiveSessions();
      }
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
      const saved = await finalizeAssistantMessage("Partial reply saved after interruption");
      if (saved) {
        await workspaceStore.refresh();
      }
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

    return () => {
      unlistenUpdate?.();
      unlistenReady?.();
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
      showModelPicker={chatProviderId === "grok"}
      modelId={chat?.model_id ?? null}
      {modelSwitching}
      onmodelchange={switchModel}
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
    <Composer disabled={!chat || sending} {sending} onsend={sendMessage} />
  </div>
</div>

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

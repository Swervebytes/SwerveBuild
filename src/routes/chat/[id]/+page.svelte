<script lang="ts">
  import { onMount } from "svelte";
  import { page } from "$app/stores";
  import { invoke } from "@tauri-apps/api/core";
  import { subscribe } from "$lib/events";
  import { goto } from "$app/navigation";
  import type { Chat, ChatMessage, MessagePart, Project } from "$lib/types";
  import { workspaceStore } from "$lib/stores/workspace.svelte";
  import { providerStore } from "$lib/stores/providers.svelte";
  import { imageSrc } from "$lib/attachments";
  import {
    type ChatUsage,
    emptyUsage,
    parseEndTurnUsage,
    parseUsageUpdate,
  } from "$lib/chatUsage";
  import ChatHeader from "$lib/components/chat/ChatHeader.svelte";
  import MessageList from "$lib/components/chat/MessageList.svelte";
  import Composer from "$lib/components/chat/Composer.svelte";
  import { browserPane } from "$lib/stores/browserPane.svelte";

  type StreamMessage = {
    id: string;
    role: "assistant" | "tool";
    content: string;
    kind?: string;
    /** ACP toolCallId — used to dedupe tool_call + tool_call_update into one chip. */
    toolCallId?: string;
  };

  let chat = $state<Chat | null>(null);
  let project = $state<Project | null>(null);
  let streaming = $state<StreamMessage[]>([]);
  let sessionReady = $state(false);
  let sending = $state(false);
  let modelSwitching = $state(false);
  let error = $state<string | null>(null);
  let activeSessionCount = $state(0);
  /** ACP-reported context window; stays empty until agent sends used+size. */
  let usage = $state<ChatUsage>(emptyUsage());

  // `-m` is a grok flag; hide the model picker when another agent backs this chat.
  const chatProviderId = $derived(chat?.provider_id ?? providerStore.active?.id ?? "grok");

  let bootstrapGen = 0;

  function resetStream() {
    streaming = [];
  }

  function applyUsage(next: ChatUsage | null) {
    if (!next) return;
    usage = next;
  }

  function appendStream(update: Record<string, unknown>) {
    const params = (update.params as Record<string, unknown>) ?? update;
    const inner = (params.update as Record<string, unknown>) ?? params;
    const sessionUpdate = String(inner.sessionUpdate ?? "");
    const text =
      ((inner.content as Record<string, unknown>)?.text as string) ??
      (inner.title as string) ??
      "";

    // ACP session-usage RFD: sessionUpdate "usage_update" with used + size.
    if (sessionUpdate === "usage_update") {
      applyUsage(parseUsageUpdate(inner));
      return;
    }

    if (!text && sessionUpdate !== "tool_call" && sessionUpdate !== "tool_call_update") return;

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
      const toolCallId =
        (typeof inner.toolCallId === "string" && inner.toolCallId) ||
        (typeof (inner.toolCall as Record<string, unknown> | undefined)?.toolCallId ===
          "string" &&
          String((inner.toolCall as Record<string, unknown>).toolCallId)) ||
        undefined;
      const label = text || (typeof inner.title === "string" ? inner.title : "") || sessionUpdate;

      if (toolCallId) {
        const idx = streaming.findIndex(
          (item) => item.role === "tool" && item.toolCallId === toolCallId,
        );
        if (idx >= 0) {
          const prev = streaming[idx];
          streaming[idx] = {
            ...prev,
            content: label || prev.content,
            kind: sessionUpdate,
          };
          streaming = [...streaming];
          return;
        }
        streaming = [
          ...streaming,
          {
            id: toolCallId,
            toolCallId,
            role: "tool",
            content: label,
            kind: sessionUpdate,
          },
        ];
        return;
      }

      streaming = [
        ...streaming,
        {
          id: crypto.randomUUID(),
          role: "tool",
          content: label,
          kind: sessionUpdate,
        },
      ];
    }
  }

  // Two paths can finalize a turn (the chat-turn-end event and the send's own
  // return); this guard keeps them from both reading `streaming` and saving the
  // same reply twice. Whichever arrives first wins, the other no-ops.
  let finalizing = false;

  async function finalizeAssistantMessage(note?: string) {
    if (finalizing) return false;
    finalizing = true;
    try {
      return await doFinalize(note);
    } finally {
      finalizing = false;
    }
  }

  async function doFinalize(note?: string) {
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

    // Persist the reasoning/tool trail with the reply. It used to be dropped on
    // save, so reloading a chat lost everything except the final prose.
    const parts: MessagePart[] = streaming
      .filter((item) => item.role === "tool" || item.kind === "thought")
      .map((item) => ({
        kind: item.role === "tool" ? ("tool" as const) : ("thought" as const),
        text: item.content,
      }));

    // Media the turn produced (Imagine renders, tool artifacts …): the backend
    // scans the WHOLE turn — prose + tool chips — for real on-disk image/video
    // paths and copies them into the attachments store. Never fatal.
    let media: { images: string[]; videos: string[] } = { images: [], videos: [] };
    const turnText = streaming.map((item) => item.content).join("\n");
    try {
      media = await invoke<{ images: string[]; videos: string[] }>("detect_chat_media", {
        chatId: chat.id,
        text: turnText,
      });
    } catch {
      /* detection is best-effort */
    }

    const saved = await invoke<ChatMessage>("append_chat_message", {
      chatId: chat.id,
      role: "assistant",
      content: text,
      images: media.images,
      videos: media.videos,
      parts,
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
    usage = emptyUsage();
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

  async function stopGenerating() {
    if (!chat) return;
    try {
      await invoke("cancel_chat_prompt", { chatId: chat.id });
    } catch (err) {
      error = String(err);
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
        parts: [],
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
    const offs = [
      subscribe<{ chatId: string; params: Record<string, unknown> }>("chat-update", (event) => {
        if (event.payload.chatId !== $page.params.id) return;
        appendStream(event.payload.params);
      }),
      subscribe<{ chatId: string }>("chat-session-ready", (event) => {
        if (event.payload.chatId === $page.params.id) {
          sessionReady = true;
        }
        refreshActiveSessions();
      }),
      subscribe<{ chatId: string; usage?: unknown }>("chat-turn-end", (event) => {
        // Arrives after the turn's last chunk event, so the saved reply includes
        // the tail that finalizing on the send's return could clip.
        if (event.payload.chatId !== $page.params.id) return;
        if (event.payload.usage !== undefined) {
          applyUsage(parseEndTurnUsage(event.payload.usage));
        }
        void finalizeAssistantMessage().then((saved) => {
          if (saved) workspaceStore.refresh();
        });
      }),
      subscribe<{ chatId: string }>("chat-session-ended", (event) => {
        // Agent process died (crash, kill, clean exit). Backend already dropped
        // the session from the map; surface a reconnect banner instead of a
        // cryptic pipe error on the next send.
        if (event.payload.chatId !== $page.params.id) return;
        sessionReady = false;
        void finalizeAssistantMessage("Partial reply saved — agent disconnected").then(
          (saved) => {
            if (saved) workspaceStore.refresh();
          },
        );
        sending = false;
        error =
          "Live agent session ended. Your saved messages are below — send a message to reconnect.";
        refreshActiveSessions();
      }),
      // Agent (or human) drove the preview browser — auto-open the dock so the
      // human sees it. Emitted by the Rust pane supervisor on navigation.
      subscribe<{ url?: string }>("browser-pane-activity", (event) => {
        browserPane.onActivity(event.payload?.url);
      }),
    ];
    return () => offs.forEach((o) => o());
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
      {usage}
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
    <Composer disabled={!chat || sending} {sending} onsend={sendMessage} onstop={stopGenerating} />
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

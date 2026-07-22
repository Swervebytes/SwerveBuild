<script lang="ts">
  import { onMount } from "svelte";
  import { page } from "$app/stores";
  import { invoke } from "@tauri-apps/api/core";
  import { subscribe } from "$lib/events";
  import { goto } from "$app/navigation";
  import type { Chat, ChatMessage, MessagePart, Project } from "$lib/types";
  import { workspaceStore } from "$lib/stores/workspace.svelte";
  import { providerStore } from "$lib/stores/providers.svelte";
  import { imageSrc, saveAcpImageBlock } from "$lib/attachments";
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
  /** Agent images this turn (ACP blocks + live path-scan). Shown as thumbs while streaming. */
  let streamImages = $state<string[]>([]);
  let streamVideos = $state<string[]>([]);
  let pathDetectTimer: ReturnType<typeof setTimeout> | null = null;
  let pathDetectGen = 0;

  // `-m` is a grok flag; hide the model picker when another agent backs this chat.
  const chatProviderId = $derived(chat?.provider_id ?? providerStore.active?.id ?? "grok");

  let bootstrapGen = 0;

  function resetStream() {
    streaming = [];
    streamImages = [];
    streamVideos = [];
    pathDetectGen += 1;
    if (pathDetectTimer) {
      clearTimeout(pathDetectTimer);
      pathDetectTimer = null;
    }
  }

  function applyUsage(next: ChatUsage | null) {
    if (!next) return;
    usage = next;
  }

  function mergeStreamMedia(images: string[], videos: string[] = []) {
    let changed = false;
    const nextImg = [...streamImages];
    const nextVid = [...streamVideos];
    for (const img of images) {
      if (img && !nextImg.includes(img)) {
        nextImg.push(img);
        changed = true;
      }
    }
    for (const vid of videos) {
      if (vid && !nextVid.includes(vid)) {
        nextVid.push(vid);
        changed = true;
      }
    }
    if (changed) {
      streamImages = nextImg;
      streamVideos = nextVid;
    }
  }

  /** Prefer real ACP image content blocks; path-scan remains a finalize fallback. */
  function harvestAcpImages(node: unknown) {
    if (!node) return;
    if (Array.isArray(node)) {
      for (const item of node) harvestAcpImages(item);
      return;
    }
    if (typeof node !== "object") return;
    const obj = node as Record<string, unknown>;
    if (obj.type === "image" && typeof obj.data === "string") {
      void saveAcpImageBlock(obj).then((path) => {
        if (path) mergeStreamMedia([path]);
      });
      return;
    }
    // Nested content blocks on tool / message updates.
    if (obj.content != null) harvestAcpImages(obj.content);
  }

  /** Flatten tool/update payloads so path-scan sees rawOutput, paths, etc. */
  function flattenForPathScan(node: unknown, depth = 0): string {
    if (depth > 6 || node == null) return "";
    if (typeof node === "string") return node;
    if (typeof node === "number" || typeof node === "boolean") return String(node);
    if (Array.isArray(node)) {
      return node.map((n) => flattenForPathScan(n, depth + 1)).filter(Boolean).join("\n");
    }
    if (typeof node === "object") {
      const obj = node as Record<string, unknown>;
      const prefer = [
        "text",
        "title",
        "path",
        "filePath",
        "file_path",
        "uri",
        "url",
        "rawOutput",
        "rawInput",
        "output",
        "content",
        "locations",
      ];
      const chunks: string[] = [];
      for (const key of prefer) {
        if (key in obj) chunks.push(flattenForPathScan(obj[key], depth + 1));
      }
      // Also walk remaining string fields lightly.
      for (const [k, v] of Object.entries(obj)) {
        if (prefer.includes(k)) continue;
        if (typeof v === "string" && v.length < 4000) chunks.push(v);
      }
      return chunks.filter(Boolean).join("\n");
    }
    return "";
  }

  /** Debounced path-scan while the agent streams so thumbs appear before finalize. */
  function schedulePathDetect(extraText = "") {
    if (!chat) return;
    const chatId = chat.id;
    if (pathDetectTimer) clearTimeout(pathDetectTimer);
    const gen = pathDetectGen;
    pathDetectTimer = setTimeout(() => {
      void (async () => {
        if (gen !== pathDetectGen || !chat || chat.id !== chatId) return;
        const turnText = [
          extraText,
          ...streaming.map((item) => item.content),
        ]
          .filter(Boolean)
          .join("\n");
        if (!turnText.trim()) return;
        try {
          const scanned = await invoke<{ images: string[]; videos: string[] }>(
            "detect_chat_media",
            { chatId, text: turnText },
          );
          if (gen !== pathDetectGen) return;
          mergeStreamMedia(scanned.images ?? [], scanned.videos ?? []);
        } catch {
          /* best-effort */
        }
      })();
    }, 280);
  }

  function appendStream(update: Record<string, unknown>) {
    const params = (update.params as Record<string, unknown>) ?? update;
    const inner = (params.update as Record<string, unknown>) ?? params;
    const sessionUpdate = String(inner.sessionUpdate ?? "");
    const content = inner.content;
    const text =
      (typeof content === "object" &&
        content !== null &&
        typeof (content as Record<string, unknown>).text === "string" &&
        String((content as Record<string, unknown>).text)) ||
      (typeof inner.title === "string" ? inner.title : "") ||
      "";

    // ACP session-usage RFD: sessionUpdate "usage_update" with used + size.
    if (sessionUpdate === "usage_update") {
      applyUsage(parseUsageUpdate(inner));
      return;
    }

    // Image content blocks + path-bearing tool payloads (S15b).
    if (
      sessionUpdate === "agent_message_chunk" ||
      sessionUpdate === "tool_call" ||
      sessionUpdate === "tool_call_update"
    ) {
      harvestAcpImages(content);
      harvestAcpImages(inner);
      const flat = flattenForPathScan(inner);
      if (flat) schedulePathDetect(flat);
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
      if (/\.(png|jpe?g|gif|webp|bmp|svg|mp4|webm|mov)\b/i.test(text)) {
        schedulePathDetect(text);
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
      // Prefer a rich label for chips; still path-scan the full flattened payload.
      const flat = flattenForPathScan(inner);
      const label =
        text ||
        (typeof inner.title === "string" ? inner.title : "") ||
        flat.slice(0, 200) ||
        sessionUpdate;

      if (toolCallId) {
        const idx = streaming.findIndex(
          (item) => item.role === "tool" && item.toolCallId === toolCallId,
        );
        if (idx >= 0) {
          const prev = streaming[idx];
          // Keep the longer of label vs prior so rawOutput paths are retained.
          const nextContent =
            flat.length > (prev.content?.length ?? 0) ? flat : label || prev.content;
          streaming[idx] = {
            ...prev,
            content: nextContent || prev.content,
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
            content: flat || label,
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
          content: flat || label,
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
    if (!chat) return false;

    let text = streaming
      .filter((item) => item.role === "assistant" && item.kind === "message")
      .map((item) => item.content)
      .join("")
      .trim();

    // Persist the reasoning/tool trail with the reply. It used to be dropped on
    // save, so reloading a chat lost everything except the final prose.
    const parts: MessagePart[] = streaming
      .filter((item) => item.role === "tool" || item.kind === "thought")
      .map((item) => ({
        kind: item.role === "tool" ? ("tool" as const) : ("thought" as const),
        text: item.content,
      }));

    // Media: stream harvest first, then final path-scan over prose + tool text
    // (S15b — agent-generated images often only appear in tool payloads).
    let media: { images: string[]; videos: string[] } = {
      images: [...streamImages],
      videos: [...streamVideos],
    };
    const turnText = streaming.map((item) => item.content).join("\n");
    try {
      const scanned = await invoke<{ images: string[]; videos: string[] }>("detect_chat_media", {
        chatId: chat.id,
        text: turnText,
      });
      for (const img of scanned.images ?? []) {
        if (!media.images.includes(img)) media.images.push(img);
      }
      for (const vid of scanned.videos ?? []) {
        if (!media.videos.includes(vid)) media.videos.push(vid);
      }
    } catch {
      /* path-scan is best-effort */
    }

    // Allow media-only turns (no prose) so generated images still save + preview.
    if (!text && media.images.length === 0 && media.videos.length === 0 && parts.length === 0) {
      return false;
    }
    if (!text && (media.images.length > 0 || media.videos.length > 0)) {
      text = ""; // bubble shows thumbs/videos even with empty prose
    }
    if (note) {
      text = text ? `${text}\n\n_${note}_` : `_${note}_`;
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

    // Paint transcript ASAP from cache; don't block on a full workspace refresh
    // or Comfy probes in the header.
    let loaded = workspaceStore.chats.find((item) => item.id === id);
    if (!loaded) {
      await workspaceStore.refresh();
      if (gen !== bootstrapGen) return;
      loaded = workspaceStore.chats.find((item) => item.id === id);
      if (!loaded) {
        try {
          loaded = await invoke<Chat>("get_chat", { chatId: id });
        } catch {
          loaded = undefined;
        }
      }
    } else {
      // Keep rail fresh without delaying first paint.
      void workspaceStore.refresh();
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

  // Only re-bootstrap when the chat id changes — not on every $page store tick.
  let lastBootstrappedId = $state<string | null>(null);
  $effect(() => {
    const id = $page.params.id;
    if (!id || id === lastBootstrappedId) return;
    lastBootstrappedId = id;
    bootstrapGen += 1;
    const gen = bootstrapGen;
    void bootstrap(id, gen);
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
    <MessageList
      messages={chat.messages}
      {streaming}
      streamImages={streamImages}
      streamVideos={streamVideos}
      {imageSrc}
    />
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

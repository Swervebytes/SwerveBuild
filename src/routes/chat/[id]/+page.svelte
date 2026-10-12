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
  import ChatHeader from "$lib/components/chat/ChatHeader.svelte";
  import ComposerDock from "$lib/components/chat/ComposerDock.svelte";
  import ProjectStatus from "$lib/components/project/ProjectStatus.svelte";
  import MessageList from "$lib/components/chat/MessageList.svelte";
  import Composer from "$lib/components/chat/Composer.svelte";
  import { browserPane } from "$lib/stores/browserPane.svelte";
  import { isChatMediaTool, mediaScanText, rememberToolName } from "$lib/chatMedia";
  import { clearChatDraft, loadChatDraft, saveChatDraft, shouldRecoverDraft } from "$lib/chatDraft";

  type StreamMessage = {
    id: string;
    role: "assistant" | "tool";
    content: string;
    kind?: string;
    /** ACP toolCallId — used to dedupe tool_call + tool_call_update into one chip. */
    toolCallId?: string;
    /** Stable tool name. Later updates replace `title` with a summary. */
    toolName?: string;
  };

  let chat = $state<Chat | null>(null);
  let project = $state<Project | null>(null);
  let streaming = $state<StreamMessage[]>([]);
  let sessionReady = $state(false);
  let sending = $state(false);
  let modelSwitching = $state(false);
  let error = $state<string | null>(null);
  /** P1.1: provider label when the last send failed on sign-in; shows the banner. */
  let authRequiredProvider = $state<string | null>(null);
  let activeSessionCount = $state(0);
  /** Agent images this turn (ACP blocks + live path-scan). Shown as thumbs while streaming. */
  let streamImages = $state<string[]>([]);
  let streamVideos = $state<string[]>([]);
  let pathDetectTimer: ReturnType<typeof setTimeout> | null = null;
  let pathDetectGen = 0;

  // `-m` is a grok flag; hide the model picker when another agent backs this chat.
  const chatProviderId = $derived(chat?.provider_id ?? providerStore.active?.id ?? "grok");

  let bootstrapGen = 0;
  /** The connect already running for this chat, so Send can wait instead of starting another grok. */
  let sessionStart: { chatId: string; gen: number; promise: Promise<void> } | null = null;

  async function ensureLiveSession(chatId: string, gen: number): Promise<void> {
    if (sessionReady && gen === bootstrapGen) return;
    if (sessionStart && sessionStart.chatId === chatId && sessionStart.gen === gen) {
      await sessionStart.promise;
      return;
    }
    const promise = invoke<{ success: boolean; message: string }>("start_chat_session", {
      chatId,
    }).then(() => {
      if (gen === bootstrapGen) sessionReady = true;
    });
    const tracked = promise.finally(() => {
      if (sessionStart?.promise === tracked) sessionStart = null;
    });
    sessionStart = { chatId, gen, promise: tracked };
    await tracked;
  }

  let draftTimer: ReturnType<typeof setTimeout> | null = null;

  function currentDraft() {
    if (!chat) return null;
    const content = streaming
      .filter((item) => item.role === "assistant" && item.kind === "message")
      .map((item) => item.content)
      .join("");
    const parts = streaming
      .filter((item) => item.role === "tool" || item.kind === "thought")
      .map((item) => ({
        kind: item.role === "tool" ? ("tool" as const) : ("thought" as const),
        text: item.content,
      }));
    if (!content.trim() && parts.length === 0 && streamImages.length === 0 && streamVideos.length === 0) {
      return null;
    }
    return {
      chatId: chat.id,
      content,
      parts,
      images: [...streamImages],
      videos: [...streamVideos],
      updatedAt: Date.now(),
    };
  }

  function persistDraftNow() {
    const draft = currentDraft();
    if (!draft) return;
    saveChatDraft(draft);
  }

  function scheduleDraft() {
    // Do not reset the timer on every token. A steady stream must still save.
    if (draftTimer) return;
    draftTimer = setTimeout(() => {
      draftTimer = null;
      persistDraftNow();
    }, 700);
  }

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

  /** Debounced path-scan of one capture-tool result. Other text is not included. */
  function schedulePathDetect(toolText: string) {
    if (!chat || !toolText.trim()) return;
    const chatId = chat.id;
    if (pathDetectTimer) clearTimeout(pathDetectTimer);
    const gen = pathDetectGen;
    pathDetectTimer = setTimeout(() => {
      void (async () => {
        if (gen !== pathDetectGen || !chat || chat.id !== chatId) return;
        try {
          const scanned = await invoke<{ images: string[]; videos: string[] }>(
            "detect_chat_media",
            { chatId, text: toolText },
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

    // Token and compaction notices are not shown, and they carry no chat text.
    if (
      sessionUpdate === "usage_update" ||
      sessionUpdate === "turn_completed" ||
      sessionUpdate === "response_completed" ||
      sessionUpdate === "auto_compact_started"
    ) {
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
      const flat = flattenForPathScan(inner);
      const label =
        text ||
        (typeof inner.title === "string" ? inner.title : "") ||
        flat.slice(0, 200) ||
        sessionUpdate;

      const pushMedia = (item: StreamMessage) => {
        if (isChatMediaTool(item.toolName) && item.content.trim()) {
          schedulePathDetect(item.content);
        }
      };

      if (toolCallId) {
        const idx = streaming.findIndex(
          (item) => item.role === "tool" && item.toolCallId === toolCallId,
        );
        if (idx >= 0) {
          const prev = streaming[idx];
          // Keep the longer of label vs prior so rawOutput paths are retained.
          const nextContent =
            flat.length > (prev.content?.length ?? 0) ? flat : label || prev.content;
          const next: StreamMessage = {
            ...prev,
            content: nextContent || prev.content,
            kind: sessionUpdate,
            toolName: rememberToolName(inner, prev.toolName),
          };
          streaming[idx] = next;
          streaming = [...streaming];
          pushMedia(next);
          return;
        }
        const created: StreamMessage = {
          id: toolCallId,
          toolCallId,
          role: "tool",
          content: flat || label,
          kind: sessionUpdate,
          toolName: rememberToolName(inner),
        };
        streaming = [...streaming, created];
        pushMedia(created);
        return;
      }

      const created: StreamMessage = {
        id: crypto.randomUUID(),
        role: "tool",
        content: flat || label,
        kind: sessionUpdate,
        toolName: rememberToolName(inner),
      };
      streaming = [...streaming, created];
      pushMedia(created);
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

    // Real image blocks are harvested while streaming. Path scan is limited
    // to capture-tool output, so a mentioned file is not shown as a picture.
    let media: { images: string[]; videos: string[] } = {
      images: [...streamImages],
      videos: [...streamVideos],
    };
    const turnText = mediaScanText(streaming);
    if (turnText.trim()) {
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
      outcome: note ? "error" : "finished",
    });
    if (draftTimer) {
      clearTimeout(draftTimer);
      draftTimer = null;
    }
    clearChatDraft(chat.id);

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

    const draft = loadChatDraft(loaded.id);
    if (draft && shouldRecoverDraft(draft, loaded.messages)) {
      try {
        const recovered = await invoke<ChatMessage>("append_chat_message", {
          chatId: loaded.id,
          role: "assistant",
          content: draft.content,
          images: draft.images ?? [],
          videos: draft.videos ?? [],
          parts: draft.parts ?? [],
        });
        if (gen !== bootstrapGen) return;
        chat = {
          ...chat,
          messages: [...chat.messages, recovered],
          updated_at: recovered.created_at,
        };
        clearChatDraft(loaded.id);
      } catch {
        /* Leave the draft. The next open can try again. */
      }
    }

    try {
      await ensureLiveSession(id, gen);
      if (gen !== bootstrapGen) return;
      await refreshActiveSessions();
    } catch (err) {
      if (gen !== bootstrapGen) return;
      sessionReady = false;
      error =
        "Live Grok session disconnected. Your saved messages are below; send a message to reconnect.";
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
      await ensureLiveSession(chat.id, bootstrapGen);
      await refreshActiveSessions();
    } catch (err) {
      sessionReady = false;
      error = `Model switch failed: ${String(err)}. Send a message to reconnect.`;
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
      const wasReady = sessionReady;
      await ensureLiveSession(chat.id, bootstrapGen);
      if (!wasReady) await refreshActiveSessions();
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
      } else if (chat) {
        try {
          await invoke("record_project_card", { chatId: chat.id, outcome: "error" });
        } catch {
          /* the card is best-effort */
        }
      }
      error = String(err);
    } finally {
      sending = false;
    }
  }

  $effect(() => {
    const count = streaming.length;
    const tail = streaming[count - 1]?.content.length ?? 0;
    if (!chat || count === 0) return;
    void tail;
    scheduleDraft();
  });

  onMount(() => {
    const flushDraft = () => persistDraftNow();
    window.addEventListener("beforeunload", flushDraft);
    const offs = [
      subscribe<{ chatId: string; params: Record<string, unknown> }>("chat-update", (event) => {
        if (event.payload.chatId !== $page.params.id) return;
        appendStream(event.payload.params);
      }),
      subscribe<{ chatId: string }>("chat-session-ready", (event) => {
        if (event.payload.chatId === $page.params.id) {
          sessionReady = true;
          // A session came up — whatever auth problem we showed is resolved.
          authRequiredProvider = null;
        }
        refreshActiveSessions();
      }),
      subscribe<{ chatId: string }>("chat-turn-end", (event) => {
        // Arrives after the turn's last chunk event, so the saved reply includes
        // the tail that finalizing on the send's return could clip.
        if (event.payload.chatId !== $page.params.id) return;
        void finalizeAssistantMessage().then((saved) => {
          if (saved) workspaceStore.refresh();
        });
      }),
      subscribe<{ chatId: string; provider: string }>("chat-auth-required", (event) => {
        // P1.1: session/new failed because the provider isn't signed in on this
        // machine. The send's catch shows the error text; this banner adds the
        // one-click route to the fix.
        if (event.payload.chatId !== $page.params.id) return;
        authRequiredProvider = event.payload.provider || "This provider";
      }),
      subscribe<{ chatId: string }>("chat-session-ended", (event) => {
        // Agent process died (crash, kill, clean exit). Backend already dropped
        // the session from the map; surface a reconnect banner instead of a
        // cryptic pipe error on the next send.
        if (event.payload.chatId !== $page.params.id) return;
        sessionReady = false;
        void finalizeAssistantMessage("Partial reply saved (agent disconnected)").then(
          (saved) => {
            if (saved) {
              workspaceStore.refresh();
            } else if (chat) {
              void invoke("record_project_card", { chatId: chat.id, outcome: "error" }).catch(
                () => {},
              );
            }
          },
        );
        sending = false;
        error =
          "Live agent session ended. Your saved messages are below; send a message to reconnect.";
        refreshActiveSessions();
      }),
      // Agent (or human) drove the preview browser — auto-open the dock so the
      // human sees it. Emitted by the Rust pane supervisor on navigation.
      subscribe<{ url?: string }>("browser-pane-activity", (event) => {
        browserPane.onActivity(event.payload?.url);
      }),
    ];
    return () => {
      window.removeEventListener("beforeunload", flushDraft);
      if (draftTimer) clearTimeout(draftTimer);
      offs.forEach((o) => o());
    };
  });
</script>

<div class="chat-page">
  <div class="reading-col rail-wrap">
    <ChatHeader
      title={chat?.title ?? "Chat"}
      projectName={project?.name}
      projectPath={project?.path}
      {activeSessionCount}
      onexport={() => invoke<string>("export_chat_markdown", { chatId: $page.params.id })}
    />
  </div>

  {#if chat && chat.messages.length === 0 && streaming.length === 0}
    <div class="reading-col status-wrap">
      <ProjectStatus projectId={chat.project_id} hideChatId={chat.id} />
    </div>
  {:else if chat}
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

  {#if authRequiredProvider}
    <div class="reading-col">
      <p class="error" data-testid="chat-auth-banner">
        {authRequiredProvider} isn't signed in on this machine.
        <a href="/settings">Open Settings → Providers</a> and use <b>Sign in</b>, then send again.
      </p>
    </div>
  {/if}

  {#if error}
    <div class="reading-col">
      <p class="error">{error}</p>
    </div>
  {/if}

  <div class="reading-col composer-wrap">
    <ComposerDock
      showModelPicker={chatProviderId === "grok"}
      modelId={chat?.model_id ?? null}
      {modelSwitching}
      connected={sessionReady}
      onmodelchange={switchModel}
    />
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

  .status-wrap {
    flex: 1;
    min-height: 0;
    overflow: auto;
    padding-top: 0.75rem;
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

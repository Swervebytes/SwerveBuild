<script lang="ts">
  import MessageBubble from "./MessageBubble.svelte";
  import ToolCallChip from "./ToolCallChip.svelte";
  import ThinkingBlock from "./ThinkingBlock.svelte";
  import type { ChatMessage } from "$lib/types";

  type StreamMessage = {
    id: string;
    role: "assistant" | "tool";
    content: string;
    kind?: string;
  };

  let {
    messages,
    streaming,
    streamImages = [],
    streamVideos = [],
    imageSrc,
  }: {
    messages: ChatMessage[];
    streaming: StreamMessage[];
    /** Live agent images (path-scan / ACP blocks) before finalize. */
    streamImages?: string[];
    streamVideos?: string[];
    imageSrc: (path: string) => string;
  } = $props();

  let scroller: HTMLDivElement | undefined = $state();
  let stick = true;

  // Audit D: an unbounded list makes a months-old chat render thousands of
  // bubbles. Render the newest window by default; one click expands fully
  // (no virtualization — the cap keeps the common case cheap and simple).
  const RENDER_CAP = 150;
  let showAll = $state(false);
  const visibleMessages = $derived(
    showAll || messages.length <= RENDER_CAP ? messages : messages.slice(-RENDER_CAP),
  );
  const hiddenCount = $derived(messages.length - visibleMessages.length);

  function onScroll() {
    if (!scroller) return;
    const gap = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight;
    stick = gap < 90;
  }

  $effect(() => {
    // re-run when messages arrive or the streaming tail grows
    void messages.length;
    void streaming.length;
    void streamImages.length;
    void streamVideos.length;
    const last = streaming[streaming.length - 1];
    void last?.content;
    if (stick && scroller) {
      scroller.scrollTop = scroller.scrollHeight;
    }
  });

  /** Index of the last streaming assistant message bubble (for live thumbs). */
  const lastAssistantStreamIdx = $derived(
    (() => {
      for (let i = streaming.length - 1; i >= 0; i--) {
        if (streaming[i].role === "assistant" && streaming[i].kind !== "thought") return i;
      }
      return -1;
    })(),
  );
</script>

<div class="scroll" bind:this={scroller} onscroll={onScroll}>
  <div class="reading-col list">
    {#if hiddenCount > 0}
      <button
        class="show-earlier"
        type="button"
        data-testid="chat-show-earlier"
        onclick={() => (showAll = true)}
      >
        Show {hiddenCount} earlier message{hiddenCount === 1 ? "" : "s"}
      </button>
    {/if}
    {#each visibleMessages as m (m.id)}
      <!-- Saved reasoning/tool trail, rendered with the same components the live
           stream uses so a reloaded chat looks like the turn did when it ran. -->
      {#each m.parts ?? [] as part, i (i)}
        {#if part.kind === "tool"}
          <ToolCallChip title={part.text} />
        {:else}
          <ThinkingBlock content={part.text} />
        {/if}
      {/each}
      <MessageBubble
        role={m.role === "user" ? "user" : "assistant"}
        content={m.content}
        images={m.images}
        videos={m.videos ?? []}
        {imageSrc}
      />
    {/each}

    {#each streaming as item, i (item.id)}
      {#if item.role === "tool"}
        <ToolCallChip title={item.content} />
      {:else if item.kind === "thought"}
        <ThinkingBlock content={item.content} />
      {:else}
        <MessageBubble
          role="assistant"
          content={item.content}
          images={i === lastAssistantStreamIdx ? streamImages : []}
          videos={i === lastAssistantStreamIdx ? streamVideos : []}
          {imageSrc}
          live
        />
      {/if}
    {/each}

    <!-- Media arrived but no assistant prose yet — still show thumbs (S15b). -->
    {#if (streamImages.length > 0 || streamVideos.length > 0) && lastAssistantStreamIdx < 0}
      <MessageBubble
        role="assistant"
        content=""
        images={streamImages}
        videos={streamVideos}
        {imageSrc}
        live
      />
    {/if}
  </div>
</div>

<style>
  .scroll {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 1rem 0 1.5rem;
  }
  .list {
    display: flex;
    flex-direction: column;
    gap: 0.4rem;
  }
  .show-earlier {
    align-self: center;
    margin-bottom: 0.5rem;
    padding: 0.3rem 0.9rem;
    font-size: 0.8rem;
    color: var(--text-muted);
    background: var(--bg-muted);
    border: 1px solid var(--border);
    border-radius: 999px;
    cursor: pointer;
  }
  .show-earlier:hover {
    color: var(--text);
  }
</style>

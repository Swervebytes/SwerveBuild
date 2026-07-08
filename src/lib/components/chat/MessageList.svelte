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
    imageSrc,
  }: {
    messages: ChatMessage[];
    streaming: StreamMessage[];
    imageSrc: (path: string) => string;
  } = $props();

  let scroller: HTMLDivElement | undefined = $state();
  let stick = true;

  function onScroll() {
    if (!scroller) return;
    const gap = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight;
    stick = gap < 90;
  }

  $effect(() => {
    // re-run when messages arrive or the streaming tail grows
    void messages.length;
    void streaming.length;
    const last = streaming[streaming.length - 1];
    void last?.content;
    if (stick && scroller) {
      scroller.scrollTop = scroller.scrollHeight;
    }
  });
</script>

<div class="scroll" bind:this={scroller} onscroll={onScroll}>
  <div class="reading-col list">
    {#each messages as m (m.id)}
      <MessageBubble
        role={m.role === "user" ? "user" : "assistant"}
        content={m.content}
        images={m.images}
        {imageSrc}
      />
    {/each}

    {#each streaming as item (item.id)}
      {#if item.role === "tool"}
        <ToolCallChip title={item.content} />
      {:else if item.kind === "thought"}
        <ThinkingBlock content={item.content} />
      {:else}
        <MessageBubble role="assistant" content={item.content} live />
      {/if}
    {/each}
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
</style>

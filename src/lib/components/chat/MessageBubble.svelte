<script lang="ts">
  import Icon from "$lib/components/ui/Icon.svelte";
  import { parseBlocks, type Inline } from "$lib/markdown";

  let {
    role,
    content,
    images = [],
    imageSrc,
    live = false,
  }: {
    role: "user" | "assistant";
    content: string;
    images?: string[];
    imageSrc?: (path: string) => string;
    live?: boolean;
  } = $props();

  type Segment = { type: "text" | "code"; content: string; lang?: string };

  // Zero-dependency fenced-code splitter (NOT a markdown parser): alternates
  // plain text and ``` code blocks so assistant code renders in a <pre>. An
  // unterminated fence (still streaming) renders as code, which is what we want.
  function parseSegments(text: string): Segment[] {
    const parts = text.split("```");
    const segs: Segment[] = [];
    for (let i = 0; i < parts.length; i++) {
      const part = parts[i];
      if (i % 2 === 0) {
        if (part) segs.push({ type: "text", content: part });
      } else {
        const nl = part.indexOf("\n");
        let lang: string | undefined;
        let body = part;
        if (nl >= 0) {
          const first = part.slice(0, nl).trim();
          if (first && !/\s/.test(first) && first.length < 24) {
            lang = first;
            body = part.slice(nl + 1);
          }
        }
        segs.push({ type: "code", content: body.replace(/\n$/, ""), lang });
      }
    }
    return segs.length ? segs : [{ type: "text", content: text }];
  }

  const segments = $derived(parseSegments(content));

  let copied = $state<number | null>(null);
  async function copy(text: string, i: number) {
    try {
      await navigator.clipboard.writeText(text);
      copied = i;
      setTimeout(() => (copied === i ? (copied = null) : null), 1400);
    } catch {
      /* ignore */
    }
  }
</script>

<article class="msg {role}">
  <div class="avatar {role}">
    <Icon name={role === "user" ? "user" : "agent"} size={16} />
  </div>
  <div class="body">
    <div class="role mono-label">{role === "user" ? "You" : "Assistant"}</div>
    <div class="content" class:live>
      {#snippet inlines(nodes: Inline[])}{#each nodes as n}{#if n.t === "text"}{n.v}{:else if n.t === "code"}<code class="md-ic">{n.v}</code>{:else if n.t === "strong"}<strong>{@render inlines(n.c)}</strong>{:else if n.t === "em"}<em>{@render inlines(n.c)}</em>{/if}{/each}{/snippet}
      {#each segments as seg, i}
        {#if seg.type === "code"}
          <div class="code">
            <div class="code-bar">
              <span class="code-lang">{seg.lang ?? "code"}</span>
              <button class="code-copy" type="button" onclick={() => copy(seg.content, i)}>
                <Icon name={copied === i ? "check" : "copy"} size={13} />
                {copied === i ? "Copied" : "Copy"}
              </button>
            </div>
            <pre><code>{seg.content}</code></pre>
          </div>
        {:else if live}
          <!-- While streaming, render plain text: markdown is still half-formed
               (open fences, partial tables) and would flicker. Finalized messages
               below get the full render. -->
          <p class="text">{seg.content}</p>
        {:else}
          <div class="md">
            {#each parseBlocks(seg.content) as b}
              {#if b.t === "h"}
                <div class="md-h" data-level={b.level}>{@render inlines(b.c)}</div>
              {:else if b.t === "p"}
                <p class="text">{#each b.lines as line, li}{#if li > 0}<br />{/if}{@render inlines(line)}{/each}</p>
              {:else if b.t === "ul"}
                <ul class="md-list">{#each b.items as it}<li>{@render inlines(it)}</li>{/each}</ul>
              {:else if b.t === "ol"}
                <ol class="md-list md-ol">{#each b.items as it}<li>{@render inlines(it)}</li>{/each}</ol>
              {:else if b.t === "quote"}
                <blockquote class="md-quote">{#each b.lines as line, li}{#if li > 0}<br />{/if}{@render inlines(line)}{/each}</blockquote>
              {:else if b.t === "table"}
                <div class="md-table-wrap">
                  <table class="md-table">
                    <thead><tr>{#each b.header as h}<th>{@render inlines(h)}</th>{/each}</tr></thead>
                    <tbody>{#each b.rows as r}<tr>{#each r as c}<td>{@render inlines(c)}</td>{/each}</tr>{/each}</tbody>
                  </table>
                </div>
              {/if}
            {/each}
          </div>
        {/if}
      {/each}
    </div>

    {#if images.length > 0 && imageSrc}
      <div class="images">
        {#each images as image}
          <img src={imageSrc(image)} alt="Attached" />
        {/each}
      </div>
    {/if}
  </div>
</article>

<style>
  .msg {
    display: grid;
    grid-template-columns: 28px 1fr;
    gap: 0.85rem;
    padding: 0.5rem 0;
  }
  .avatar {
    width: 28px;
    height: 28px;
    border-radius: 8px;
    display: grid;
    place-items: center;
    flex: none;
    margin-top: 0.15rem;
  }
  .avatar.user {
    background: var(--bg-surface-2);
    border: 1px solid var(--border);
    color: var(--text-secondary);
  }
  .avatar.assistant {
    background: var(--sc-accent-tint);
    color: var(--sc-accent);
    border: 1px solid color-mix(in srgb, var(--sc-accent) 30%, transparent);
  }

  .body {
    min-width: 0;
  }
  .role {
    margin-bottom: 0.3rem;
  }
  .content {
    display: flex;
    flex-direction: column;
    gap: 0.6rem;
  }

  .text {
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    font-size: 0.9375rem;
    line-height: 1.6;
    color: var(--text-primary);
  }
  .user .text {
    color: var(--text-secondary);
  }

  .live .text:last-child::after {
    content: "▍";
    color: var(--sc-accent);
    margin-left: 1px;
    animation: blink 1s steps(2, start) infinite;
  }
  @keyframes blink {
    50% {
      opacity: 0;
    }
  }

  .code {
    border: 1px solid var(--border);
    border-radius: var(--radius);
    overflow: hidden;
    background: var(--bg-muted);
  }
  .code-bar {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 0.35rem 0.6rem;
    border-bottom: 1px solid var(--border);
    background: var(--bg-surface-2);
  }
  .code-lang {
    font-family: var(--font-mono);
    font-size: 0.6875rem;
    color: var(--text-muted);
    text-transform: uppercase;
    letter-spacing: 0.06em;
  }
  .code-copy {
    display: inline-flex;
    align-items: center;
    gap: 0.35rem;
    border: 0;
    background: transparent;
    color: var(--text-muted);
    font-size: 0.75rem;
    cursor: pointer;
    padding: 0.15rem 0.3rem;
    border-radius: 6px;
    transition: color var(--dur-fast) var(--ease);
  }
  .code-copy:hover {
    color: var(--text-primary);
  }
  pre {
    margin: 0;
    padding: 0.75rem 0.85rem;
    overflow-x: auto;
    font-family: var(--font-mono);
    font-size: 0.8125rem;
    line-height: 1.55;
    color: var(--text-primary);
  }

  .images {
    display: flex;
    gap: 0.5rem;
    margin-top: 0.6rem;
    flex-wrap: wrap;
  }
  .images img {
    max-width: 180px;
    max-height: 120px;
    border-radius: var(--radius-sm);
    border: 1px solid var(--border);
  }

  /* Rendered markdown (finalized assistant/user messages) */
  .md {
    display: flex;
    flex-direction: column;
    gap: 0.55rem;
    min-width: 0;
  }
  .md-h {
    font-weight: 650;
    color: var(--text-primary);
    line-height: 1.3;
    margin-top: 0.15rem;
  }
  .md-h[data-level="1"] {
    font-size: 1.15rem;
  }
  .md-h[data-level="2"] {
    font-size: 1.05rem;
  }
  .md-h[data-level="3"] {
    font-size: 0.975rem;
  }
  .md-h[data-level="4"],
  .md-h[data-level="5"],
  .md-h[data-level="6"] {
    font-size: 0.9rem;
    color: var(--text-secondary);
    text-transform: none;
  }
  .md-list {
    margin: 0;
    padding-left: 1.3rem;
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
    font-size: 0.9375rem;
    line-height: 1.55;
    color: var(--text-primary);
  }
  .user .md-list {
    color: var(--text-secondary);
  }
  .md-list li {
    padding-left: 0.15rem;
  }
  .md-ic {
    font-family: var(--font-mono);
    font-size: 0.85em;
    background: var(--bg-muted);
    border: 1px solid var(--border);
    border-radius: 5px;
    padding: 0.04rem 0.3rem;
    overflow-wrap: anywhere;
  }
  .md-quote {
    margin: 0;
    padding: 0.15rem 0 0.15rem 0.8rem;
    border-left: 3px solid var(--border);
    color: var(--text-secondary);
    font-size: 0.9375rem;
    line-height: 1.55;
  }
  .md-table-wrap {
    overflow-x: auto;
    max-width: 100%;
  }
  .md-table {
    border-collapse: collapse;
    font-size: 0.875rem;
    line-height: 1.5;
  }
  .md-table th,
  .md-table td {
    border: 1px solid var(--border);
    padding: 0.3rem 0.6rem;
    text-align: left;
    vertical-align: top;
  }
  .md-table th {
    background: var(--bg-surface-2);
    font-weight: 600;
    color: var(--text-primary);
  }
  .md-table td {
    color: var(--text-secondary);
  }
</style>

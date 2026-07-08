<script lang="ts">
  import Icon from "$lib/components/ui/Icon.svelte";
  import { pasteToImages } from "$lib/attachments";

  let {
    disabled = false,
    sending = false,
    placeholder = "Ask Grok to build, fix, or explain…  ⏎ send · ⇧⏎ newline · paste an image",
    onsend,
  }: {
    disabled?: boolean;
    sending?: boolean;
    placeholder?: string;
    onsend: (text: string, images: string[]) => void;
  } = $props();

  let draft = $state("");
  let pendingImages = $state<string[]>([]);
  let el: HTMLTextAreaElement | undefined = $state();

  const canSend = $derived(
    !disabled && !sending && (draft.trim().length > 0 || pendingImages.length > 0),
  );

  function grow() {
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, 220)}px`;
  }

  function submit() {
    if (!canSend) return;
    const text = draft.trim();
    const images = [...pendingImages];
    onsend(text, images);
    draft = "";
    pendingImages = [];
    queueMicrotask(grow);
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
      e.preventDefault();
      submit();
    }
  }

  async function onPaste(e: ClipboardEvent) {
    const added = await pasteToImages(e);
    if (added.length) pendingImages = [...pendingImages, ...added];
  }

  function removeImage(path: string) {
    pendingImages = pendingImages.filter((p) => p !== path);
  }
</script>

<div class="composer">
  {#if pendingImages.length > 0}
    <div class="attachments">
      {#each pendingImages as image, i}
        <div class="attachment">
          <Icon name="image" size={13} />
          <span>Image {i + 1}</span>
          <button type="button" class="rm" onclick={() => removeImage(image)} aria-label="Remove">
            <Icon name="close" size={12} />
          </button>
        </div>
      {/each}
    </div>
  {/if}

  <div class="input-row">
    <textarea
      bind:this={el}
      class="input"
      {placeholder}
      bind:value={draft}
      oninput={grow}
      onkeydown={onKeydown}
      onpaste={onPaste}
      rows="1"
    ></textarea>
    <button
      class="send"
      type="button"
      onclick={submit}
      disabled={!canSend}
      aria-label="Send"
      title="Send (Enter)"
    >
      <Icon name="send" size={17} />
    </button>
  </div>
</div>

<style>
  .composer {
    flex-shrink: 0;
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    background: var(--bg-surface);
    padding: 0.6rem 0.6rem 0.6rem 0.85rem;
    box-shadow: var(--shadow-md);
    transition: border-color var(--dur) var(--ease);
  }
  .composer:focus-within {
    border-color: color-mix(in srgb, var(--sc-accent) 45%, var(--border));
  }

  .attachments {
    display: flex;
    flex-wrap: wrap;
    gap: 0.4rem;
    margin-bottom: 0.5rem;
  }
  .attachment {
    display: inline-flex;
    align-items: center;
    gap: 0.35rem;
    padding: 0.25rem 0.5rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-pill);
    background: var(--bg-muted);
    font-size: 0.75rem;
    color: var(--text-secondary);
  }
  .rm {
    display: inline-flex;
    border: 0;
    background: transparent;
    color: var(--text-muted);
    cursor: pointer;
    padding: 0;
  }
  .rm:hover {
    color: var(--text-primary);
  }

  .input-row {
    display: flex;
    align-items: flex-end;
    gap: 0.5rem;
  }
  .input {
    flex: 1;
    border: 0;
    outline: none;
    resize: none;
    background: transparent;
    color: var(--text-primary);
    font-size: 0.9375rem;
    line-height: 1.55;
    padding: 0.35rem 0;
    max-height: 220px;
    overflow-y: auto;
  }
  .input::placeholder {
    color: var(--text-faint);
  }

  .send {
    display: grid;
    place-items: center;
    width: 2.2rem;
    height: 2.2rem;
    flex: none;
    border: 0;
    border-radius: var(--radius);
    background: var(--sc-accent);
    color: #04060d;
    cursor: pointer;
    transition:
      box-shadow var(--dur-fast) var(--ease),
      opacity var(--dur-fast) var(--ease),
      transform var(--dur-fast) var(--ease);
  }
  .send:hover:not(:disabled) {
    box-shadow: var(--glow) var(--sc-accent);
  }
  .send:active:not(:disabled) {
    transform: translateY(0.5px);
  }
  .send:disabled {
    opacity: 0.4;
    cursor: not-allowed;
  }
</style>

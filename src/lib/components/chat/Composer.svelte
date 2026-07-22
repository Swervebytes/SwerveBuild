<script lang="ts">
  import Icon from "$lib/components/ui/Icon.svelte";
  import {
    imageSrc,
    pasteToImages,
    persistImageFiles,
    pickImageAttachments,
  } from "$lib/attachments";

  let {
    disabled = false,
    sending = false,
    placeholder = "Ask Grok to build, fix, or explain…  ⏎ send · ⇧⏎ newline · paste, drop, or attach an image",
    onsend,
    onstop,
  }: {
    disabled?: boolean;
    sending?: boolean;
    placeholder?: string;
    onsend: (text: string, images: string[]) => void;
    onstop?: () => void;
  } = $props();

  let draft = $state("");
  let pendingImages = $state<string[]>([]);
  let el: HTMLTextAreaElement | undefined = $state();
  let dragOver = $state(false);
  let attaching = $state(false);
  let attachError = $state<string | null>(null);

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
    attachError = null;
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

  async function onPickFiles() {
    if (disabled || sending || attaching) return;
    attaching = true;
    attachError = null;
    try {
      const added = await pickImageAttachments();
      if (added.length) pendingImages = [...pendingImages, ...added];
    } catch (err) {
      attachError = String(err);
    } finally {
      attaching = false;
    }
  }

  function onDragEnter(e: DragEvent) {
    if (disabled || sending) return;
    if (!e.dataTransfer?.types?.includes("Files")) return;
    e.preventDefault();
    dragOver = true;
  }

  function onDragOver(e: DragEvent) {
    if (!dragOver && e.dataTransfer?.types?.includes("Files")) {
      e.preventDefault();
      dragOver = true;
    } else if (dragOver) {
      e.preventDefault();
    }
  }

  function onDragLeave(e: DragEvent) {
    // Only clear when leaving the composer shell (not child nodes).
    const related = e.relatedTarget as Node | null;
    if (related && (e.currentTarget as HTMLElement).contains(related)) return;
    dragOver = false;
  }

  async function onDrop(e: DragEvent) {
    e.preventDefault();
    dragOver = false;
    if (disabled || sending) return;
    const files = e.dataTransfer?.files;
    if (!files?.length) return;
    attaching = true;
    attachError = null;
    try {
      const added = await persistImageFiles(files);
      if (added.length) {
        pendingImages = [...pendingImages, ...added];
      } else {
        attachError = "Drop images only (png, jpg, gif, webp, …)";
      }
    } catch (err) {
      attachError = String(err);
    } finally {
      attaching = false;
    }
  }
</script>

<div
  class="composer"
  class:drag-over={dragOver}
  ondragenter={onDragEnter}
  ondragover={onDragOver}
  ondragleave={onDragLeave}
  ondrop={onDrop}
  role="group"
  aria-label="Message composer"
>
  {#if dragOver}
    <div class="drop-hint" aria-hidden="true">Drop images to attach</div>
  {/if}

  {#if pendingImages.length > 0}
    <div class="attachments">
      {#each pendingImages as image, i (image)}
        <div class="attachment">
          <img class="thumb" src={imageSrc(image)} alt="Attachment {i + 1}" />
          <span class="att-label">Image {i + 1}</span>
          <button type="button" class="rm" onclick={() => removeImage(image)} aria-label="Remove">
            <Icon name="close" size={12} />
          </button>
        </div>
      {/each}
    </div>
  {/if}

  {#if attachError}
    <p class="attach-error">{attachError}</p>
  {/if}

  <div class="input-row">
    <button
      class="attach"
      type="button"
      onclick={onPickFiles}
      disabled={disabled || sending || attaching}
      aria-label="Attach images"
      title="Attach images"
      data-testid="composer-attach"
    >
      <Icon name="image" size={16} />
    </button>
    <textarea
      bind:this={el}
      class="input"
      {placeholder}
      bind:value={draft}
      oninput={grow}
      onkeydown={onKeydown}
      onpaste={onPaste}
      rows="1"
      disabled={disabled}
    ></textarea>
    {#if sending && onstop}
      <button
        class="send stop"
        type="button"
        onclick={onstop}
        aria-label="Stop generating"
        title="Stop generating"
      >
        <Icon name="close" size={16} />
      </button>
    {:else}
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
    {/if}
  </div>
</div>

<style>
  .composer {
    position: relative;
    flex-shrink: 0;
    border: 1px solid var(--border);
    border-radius: var(--radius-lg);
    background: var(--bg-surface);
    padding: 0.6rem 0.6rem 0.6rem 0.85rem;
    box-shadow: var(--shadow-md);
    transition:
      border-color var(--dur) var(--ease),
      background var(--dur-fast) var(--ease);
  }
  .composer:focus-within {
    border-color: color-mix(in srgb, var(--sc-accent) 45%, var(--border));
  }
  .composer.drag-over {
    border-color: var(--sc-accent);
    background: color-mix(in srgb, var(--sc-accent) 8%, var(--bg-surface));
  }

  .drop-hint {
    position: absolute;
    inset: 0;
    z-index: 2;
    display: grid;
    place-items: center;
    border-radius: inherit;
    background: color-mix(in srgb, var(--sc-accent) 12%, transparent);
    color: var(--sc-accent);
    font-size: 0.8125rem;
    font-weight: 600;
    pointer-events: none;
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
    padding: 0.2rem 0.4rem 0.2rem 0.2rem;
    border: 1px solid var(--border);
    border-radius: var(--radius-pill);
    background: var(--bg-muted);
    font-size: 0.75rem;
    color: var(--text-secondary);
  }
  .thumb {
    width: 28px;
    height: 28px;
    border-radius: 50%;
    object-fit: cover;
    background: var(--bg-surface-2);
    flex: none;
  }
  .att-label {
    max-width: 5rem;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
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

  .attach-error {
    margin: 0 0 0.4rem;
    font-size: 0.75rem;
    color: var(--danger);
  }

  .input-row {
    display: flex;
    align-items: flex-end;
    gap: 0.4rem;
  }
  .attach {
    display: grid;
    place-items: center;
    width: 2.2rem;
    height: 2.2rem;
    flex: none;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--bg-surface);
    color: var(--text-secondary);
    cursor: pointer;
    transition:
      color var(--dur-fast) var(--ease),
      border-color var(--dur-fast) var(--ease);
  }
  .attach:hover:not(:disabled) {
    color: var(--sc-accent);
    border-color: var(--sc-accent);
  }
  .attach:disabled {
    opacity: 0.4;
    cursor: not-allowed;
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
  .input:disabled {
    opacity: 0.6;
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
  .send.stop {
    background: var(--bg-surface-2);
    border: 1px solid var(--border);
    color: var(--text-primary);
    opacity: 1;
  }
  .send.stop:hover {
    border-color: var(--danger);
    color: var(--danger);
    box-shadow: none;
  }
</style>

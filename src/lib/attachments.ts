import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";

// Render a stored attachment path through Tauri's asset protocol.
export function imageSrc(path: string): string {
  return convertFileSrc(path);
}

const IMAGE_MIME_PREFIX = "image/";
const IMAGE_EXTS = new Set(["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg"]);

export function isImageFileName(name: string): boolean {
  const ext = name.split(".").pop()?.toLowerCase() ?? "";
  return IMAGE_EXTS.has(ext);
}

// Pull any image items out of a paste event, persist them, and return their
// stored paths. Non-image pastes are left untouched.
export async function pasteToImages(event: ClipboardEvent): Promise<string[]> {
  const items = event.clipboardData?.items;
  if (!items) return [];

  const paths: string[] = [];
  for (const item of items) {
    if (!item.type.startsWith(IMAGE_MIME_PREFIX)) continue;
    event.preventDefault();
    const file = item.getAsFile();
    if (!file) continue;
    const path = await persistImageFile(file);
    if (path) paths.push(path);
  }
  return paths;
}

/** Persist a browser File (paste / drag-drop) into the attachments dir. */
export async function persistImageFile(file: File): Promise<string | null> {
  if (!file.type.startsWith(IMAGE_MIME_PREFIX) && !isImageFileName(file.name)) {
    return null;
  }
  const dataUrl = await readFileAsDataUrl(file);
  return invoke<string>("save_pasted_image", { dataUrl });
}

export async function persistImageFiles(files: FileList | File[]): Promise<string[]> {
  const list = Array.from(files);
  const paths: string[] = [];
  for (const file of list) {
    try {
      const path = await persistImageFile(file);
      if (path) paths.push(path);
    } catch {
      /* skip failed files */
    }
  }
  return paths;
}

/** Native file picker → copy into attachments (paths stay in-scope for asset protocol). */
export async function pickImageAttachments(): Promise<string[]> {
  const selected = await open({
    multiple: true,
    title: "Attach images",
    filters: [
      {
        name: "Images",
        extensions: ["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg"],
      },
    ],
  });
  if (!selected) return [];
  const paths = Array.isArray(selected) ? selected : [selected];
  if (paths.length === 0) return [];
  return invoke<string[]>("import_attachment_files", { paths });
}

/** Persist an ACP image content block (base64 + mime) into attachments. */
export async function saveAcpImageBlock(block: {
  data?: unknown;
  mimeType?: unknown;
  mime_type?: unknown;
}): Promise<string | null> {
  const data = typeof block.data === "string" ? block.data : null;
  if (!data) return null;
  const mimeRaw =
    (typeof block.mimeType === "string" && block.mimeType) ||
    (typeof block.mime_type === "string" && block.mime_type) ||
    "image/png";
  const mime = mimeRaw.includes("/") ? mimeRaw : `image/${mimeRaw}`;
  const dataUrl = data.startsWith("data:") ? data : `data:${mime};base64,${data}`;
  try {
    return await invoke<string>("save_pasted_image", { dataUrl });
  } catch {
    return null;
  }
}

function readFileAsDataUrl(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result));
    reader.onerror = () => reject(reader.error ?? new Error("read failed"));
    reader.readAsDataURL(file);
  });
}

import { convertFileSrc, invoke } from "@tauri-apps/api/core";

// Render a stored attachment path through Tauri's asset protocol.
export function imageSrc(path: string): string {
  return convertFileSrc(path);
}

// Pull any image items out of a paste event, persist them, and return their
// stored paths. Non-image pastes are left untouched.
export async function pasteToImages(event: ClipboardEvent): Promise<string[]> {
  const items = event.clipboardData?.items;
  if (!items) return [];

  const paths: string[] = [];
  for (const item of items) {
    if (!item.type.startsWith("image/")) continue;
    event.preventDefault();
    const file = item.getAsFile();
    if (!file) continue;

    const dataUrl = await new Promise<string>((resolve, reject) => {
      const reader = new FileReader();
      reader.onload = () => resolve(String(reader.result));
      reader.onerror = () => reject(reader.error);
      reader.readAsDataURL(file);
    });

    paths.push(await invoke<string>("save_pasted_image", { dataUrl }));
  }
  return paths;
}

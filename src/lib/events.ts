import { listen, type EventCallback } from "@tauri-apps/api/event";

// Unmount-safe Tauri event subscription.
//
// `listen()` resolves its unlisten function asynchronously. The naive
// `listen(...).then((u) => (unlisten = u))` pattern leaks when a component
// unmounts before that promise resolves: the sync cleanup runs while `unlisten`
// is still null, so the handler registers after teardown and is never removed.
// This returns a synchronous disposer that unlistens the moment it can — either
// immediately if already resolved, or by flagging cancellation so the pending
// listener detaches itself as soon as it arrives.
export function subscribe<T>(event: string, handler: EventCallback<T>): () => void {
  let cancelled = false;
  let off: (() => void) | null = null;
  listen<T>(event, handler)
    .then((u) => {
      if (cancelled) u();
      else off = u;
    })
    .catch(() => {});
  return () => {
    cancelled = true;
    off?.();
    off = null;
  };
}

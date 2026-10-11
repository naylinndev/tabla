/**
 * Copies text that still has to be loaded (a schema, a CREATE statement).
 *
 * WebKit (the macOS webview) only lets a page write to the clipboard during
 * the click itself; after awaiting a database query it throws NotAllowedError.
 * So the clipboard is claimed right away with a ClipboardItem whose content
 * arrives later. Call this synchronously from the click handler, before any
 * await. Rejects with the loader's own error if loading fails.
 */
export async function copyWhenReady(load: Promise<string>): Promise<void> {
  if (typeof ClipboardItem !== "undefined" && navigator.clipboard?.write) {
    const blob = load.then((text) => new Blob([text], { type: "text/plain" }));
    try {
      await navigator.clipboard.write([new ClipboardItem({ "text/plain": blob })]);
      return;
    } catch {
      // Either loading failed (surface that error below) or this engine
      // doesn't take a pending ClipboardItem; fall back to a plain write.
    }
  }
  await navigator.clipboard.writeText(await load);
}

/**
 * DEV ONLY. When the UI runs in a plain browser (not inside Tauri) and the
 * URL carries `?bridge=<token>`, route `invoke()` to the debug-only
 * `tabla bridge` server so the real backend can be tested end-to-end.
 * Stripped from production builds by the `import.meta.env.DEV` guard.
 */
export function installDevBridge() {
  if (!import.meta.env.DEV || "__TAURI_INTERNALS__" in window) return;
  const token = new URLSearchParams(location.search).get("bridge");
  if (!token) return;
  (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = {
    async invoke(cmd: string, args?: Record<string, unknown>) {
      const res = await fetch("http://127.0.0.1:1430", {
        method: "POST",
        headers: { "content-type": "application/json", "x-tabla-token": token },
        body: JSON.stringify({ cmd, args: args ?? {} }),
      });
      const body = await res.json();
      if ("err" in body) throw body.err;
      return body.ok;
    },
    transformCallback: () => 0,
    metadata: { currentWindow: { label: "main" }, currentWebview: { label: "main" } },
  };
  console.info("[tabla] dev bridge active");
}

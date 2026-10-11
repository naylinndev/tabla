import React from "react";
import ReactDOM from "react-dom/client";
// All fonts are bundled (free, OFL-licensed Google Fonts) so the app works offline.
import "@fontsource-variable/geist";
import "@fontsource-variable/geist-mono";
import "@fontsource-variable/inter";
import "@fontsource/arimo/400.css";
import "@fontsource/arimo/500.css";
import "@fontsource/arimo/600.css";
import "@fontsource/arimo/700.css";
import "./styles/app.css";
import { installDevBridge } from "./lib/devBridge";
import { hydrateLocal } from "./lib/local";

installDevBridge();

// Same test as isMac in components/ui, which can't be imported before the store is ready.
if (/Mac/i.test(navigator.platform || navigator.userAgent)) document.documentElement.classList.add("mac");

// The store reads saved state when it's created, so load it before the app.
hydrateLocal().then(async () => {
  const { default: App } = await import("./App");
  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>,
  );
});

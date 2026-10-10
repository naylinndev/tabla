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
import App from "./App";
import { isMac } from "./components/ui";
import { installDevBridge } from "./lib/devBridge";

installDevBridge();

if (isMac) document.documentElement.classList.add("mac");

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);

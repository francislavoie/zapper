import React from "react";
import { createRoot } from "react-dom/client";
import "./styles.css";
import { App } from "./App";
import { backendKind } from "./backend";
import { WebGate } from "./components/WebGate";
import { StoreProvider } from "./store";
import { loadZoom, applyZoom } from "./zoom";

// Before the first render, not in an effect after it: the remembered size is
// the size this app opens at, with no frame at 100% in between.
applyZoom(loadZoom());

// For styles.css: a browser page has no window behind it to be dark, so it
// paints its own background (the app's window must not; see styles.css).
document.documentElement.dataset.backend = backendKind;

const app = (
  <StoreProvider>
    <App />
  </StoreProvider>
);

createRoot(document.getElementById("root")!).render(
  <React.StrictMode>{backendKind === "web" ? <WebGate>{app}</WebGate> : app}</React.StrictMode>,
);

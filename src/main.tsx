import React from "react";
import { createRoot } from "react-dom/client";
import "./styles.css";
import { App } from "./App";
import { StoreProvider } from "./store";
import { loadZoom, applyZoom } from "./zoom";

// Before the first render, not in an effect after it: the remembered size is
// the size this app opens at, with no frame at 100% in between.
applyZoom(loadZoom());

createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <StoreProvider>
      <App />
    </StoreProvider>
  </React.StrictMode>,
);

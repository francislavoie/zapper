import React from "react";
import { createRoot } from "react-dom/client";
import "./styles.css";

// Placeholder shell. The browse UI lands in a later plan; what matters now is
// that the page paints nothing opaque, so the mpv surface below stays visible.
function App() {
  return <div className="shell">zapper</div>;
}

createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);

import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 5180,
    // Refuse to start rather than quietly moving to 5181: tauri.conf.json
    // points `devUrl` at this exact port, so a silent move means the window
    // opens on whatever was already there — or on nothing.
    strictPort: true,
    watch: {
      // The Rust tree is cargo's business. Watching it means a file-change
      // storm every time `cargo` writes to target/, and page reloads
      // triggered by a rebuild that has nothing to do with the frontend.
      ignored: ["**/src-tauri/**"],
    },
  },
  build: { outDir: "dist", emptyOutDir: true },
});

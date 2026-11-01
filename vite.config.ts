import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { licenseNotices } from "./tools/licenses.mjs";

export default defineConfig({
  // licenseNotices: the production build writes dist/licenses/ (the npm packages' notices, with
  // zapper's LICENSE, libmpv's notice and the Rust crates' beside them) for Settings > About and
  // the packages, and fails on a license outside about.toml's allow-list. docs/licensing.md.
  plugins: [react(), licenseNotices()],
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

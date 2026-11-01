import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { licenseNotices } from "./tools/licenses.mjs";

// Node's environment, without Node's types (the config is type-checked with the app's).
const env =
  (globalThis as { process?: { env: Record<string, string | undefined> } }).process?.env ?? {};
const zapperServer = env.ZAPPER_SERVER ?? "http://127.0.0.1:8796";

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
    // `VITE_ZAPPER_BACKEND=web npm run dev` runs the interface against a running zapper-server
    // instead of the mock (src/backend.ts): its API is passed through, so the page and the API
    // share an origin as they do when the service serves the page itself. ZAPPER_SERVER says
    // where the service is. The proxy names the service as the host it asks, and the page's
    // origin as the same: the service refuses a change from any other (web.rs's csrf).
    proxy:
      env.VITE_ZAPPER_BACKEND === "web"
        ? { "/api": { target: zapperServer, changeOrigin: true, headers: { origin: zapperServer } } }
        : undefined,
    watch: {
      // The Rust tree is cargo's business. Watching it means a file-change
      // storm every time `cargo` writes to target/, and page reloads
      // triggered by a rebuild that has nothing to do with the frontend.
      ignored: ["**/src-tauri/**"],
    },
  },
  build: { outDir: "dist", emptyOutDir: true },
});

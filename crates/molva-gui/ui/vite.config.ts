// SPDX-License-Identifier: MIT
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// The port is fixed: `devUrl` in tauri.conf.json expects the same port
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    // styles.css reads the DDLC kit that the repository vendors for the site: the window and
    // the site must share one pin of the palette, the roles and the two faces. The server
    // needs the repository root to read it, because it lies above the frontend's own root
    fs: { allow: ["../../.."] },
  },
  build: {
    target: "es2022",
    // The build goes inside the application, so it needs no source maps outside
    sourcemap: false,
  },
});

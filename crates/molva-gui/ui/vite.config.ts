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
  },
  build: {
    target: "es2022",
    // The build goes inside the application, so it needs no source maps outside
    sourcemap: false,
  },
});

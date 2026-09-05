// SPDX-License-Identifier: MIT
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// Порт фиксирован: его же ждёт `devUrl` в tauri.conf.json.
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
  },
  build: {
    target: "es2022",
    // Сборка попадает внутрь приложения, карты кода наружу не нужны.
    sourcemap: false,
  },
});

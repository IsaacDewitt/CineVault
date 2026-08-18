import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// https://vite.dev/config/
export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
    watch: {
      ignored: ["**/src-tauri/**", "**/target/**"],
    },
  },
  build: {
    target: "es2021",
    chunkSizeWarningLimit: 1500,
  },
});

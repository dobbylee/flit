import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

const devHost = process.env.TAURI_DEV_HOST;

export default defineConfig({
  clearScreen: false,
  plugins: [react()],
  server: {
    host: devHost || false,
    port: 1420,
    strictPort: true,
    watch: {
      ignored: ["**/src-tauri/**"],
    },
    hmr: devHost
      ? {
          host: devHost,
          port: 1421,
          protocol: "ws",
        }
      : undefined,
  },
});

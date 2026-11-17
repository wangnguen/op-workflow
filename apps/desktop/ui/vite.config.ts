import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

// Tauri loads http://localhost:5173 in dev and ../ui/dist in release builds.
export default defineConfig({
  base: "./",
  plugins: [react()],
  clearScreen: false,
  server: { port: 5173, strictPort: true },
  // one chunk read from disk by the Tauri window: size does not slow loading, so warn later
  build: { outDir: "dist", target: "es2022", sourcemap: false, chunkSizeWarningLimit: 800 },
  test: { environment: "node", include: ["src/**/*.test.ts"] },
});

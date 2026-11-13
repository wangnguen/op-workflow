import react from "@vitejs/plugin-react";
import { defineConfig } from "vitest/config";

// Tauri loads http://localhost:5173 in dev and ../ui/dist in release builds.
export default defineConfig({
  base: "./",
  plugins: [react()],
  clearScreen: false,
  server: { port: 5173, strictPort: true },
  build: { outDir: "dist", target: "es2022", sourcemap: false },
  test: { environment: "node", include: ["src/**/*.test.ts"] },
});

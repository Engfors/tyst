import { resolve } from "node:path";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { defineConfig } from "vite";

// One page per window: meeting, pill, settings, onboarding.
export default defineConfig({
  plugins: [svelte()],
  clearScreen: false,
  server: { port: 5173, strictPort: true },
  build: {
    target: "safari15",
    outDir: "dist",
    emptyOutDir: true,
    rollupOptions: {
      input: {
        meeting: resolve(__dirname, "meeting.html"),
        pill: resolve(__dirname, "pill.html"),
        settings: resolve(__dirname, "settings.html"),
        onboarding: resolve(__dirname, "onboarding.html"),
      },
    },
  },
});

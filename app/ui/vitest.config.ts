import { svelteTesting } from "@testing-library/svelte/vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";
import { defineConfig } from "vitest/config";

// Component tests for the meeting window. The pure key policy stays on `node --test`
// (`naming-action.test.ts`); both run from `npm run check`. `browser` selects Svelte's
// client build; the default condition is the server build, which cannot mount.
export default defineConfig({
  plugins: [svelte({ hot: false }), svelteTesting()],
  resolve: {
    conditions: ["browser"],
  },
  test: {
    environment: "jsdom",
    include: ["src/**/*.component.test.ts"],
  },
});

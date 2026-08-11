import { defineConfig } from "vitest/config";

// Unit tests for pure TS logic (parsers, formatters). Kept separate from
// vite.config.js so these run without the SvelteKit plugin — they are plain
// node tests, not component tests.
export default defineConfig({
  test: {
    include: ["src/**/*.test.ts"],
    environment: "node",
  },
});

import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// Component tests need a DOM; the store/format tests do not care either way.
export default defineConfig({
  plugins: [react()],
  test: {
    environment: "jsdom",
    globals: false,
    // Vitest's 5s default is not a margin here, it is a coin toss. The board
    // tests render the whole app into jsdom under fake timers, and the first
    // one in a file also pays for the module graph: the slowest of them clocks
    // ~4–5s before any of it is the test's own fault. A few hundred
    // milliseconds — one more module, a machine under load — then turns the
    // suite red for a reason that has nothing to do with the code under test.
    // Fifteen seconds is still short enough to catch a genuine hang.
    testTimeout: 15_000,
  },
});

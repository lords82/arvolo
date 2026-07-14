import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// Component tests need a DOM; the store/format tests do not care either way.
export default defineConfig({
  plugins: [react()],
  test: {
    environment: "jsdom",
    globals: false,
  },
});

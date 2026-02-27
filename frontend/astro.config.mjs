// @ts-check
import { defineConfig } from "astro/config";

import svelte from "@astrojs/svelte";

import tailwindcss from "@tailwindcss/vite";

// https://astro.build/config
export default defineConfig({
  integrations: [svelte()],
  outDir: "../static",

  vite: {
    plugins: [tailwindcss()],
    server: {
      allowedHosts: true,
    },
  },

  server: {
    // Allow specific hosts for the dev server
    allowedHosts: ["test.home"],
  },
});

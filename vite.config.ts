import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// @ts-expect-error process is a nodejs global
const host = process.env.TAURI_DEV_HOST;

// https://vite.dev/config/
export default defineConfig(async () => ({
  plugins: [react()],

  // Options Vite pour Tauri, utilisées aussi bien par `npm run dev` que par `tauri dev`/`tauri build`.
  //
  // 1. Ne pas masquer les erreurs Rust en effaçant l'écran.
  clearScreen: false,
  // 2. Tauri exige un port fixe : échouer plutôt qu'en choisir un autre.
  server: {
    port: 1420,
    strictPort: true,
    host: host || false,
    hmr: host
      ? {
          protocol: "ws",
          host,
          port: 1421,
        }
      : undefined,
    watch: {
      // 3. Ignorer `src-tauri`, sans rapport avec le build front.
      ignored: ["**/src-tauri/**"],
    },
  },
}));

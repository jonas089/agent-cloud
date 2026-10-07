import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// `npm run dev` serves the UI with hot reload and forwards `/api` to a market, so the browser
// sees one origin. `MARKET=http://host:port npm run dev` points it at another market.
export default defineConfig({
  plugins: [react()],
  server: {
    host: true,
    proxy: { "/api": { target: process.env.MARKET ?? "http://127.0.0.1:8420", changeOrigin: true } },
  },
});

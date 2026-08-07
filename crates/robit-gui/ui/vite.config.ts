import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

export default defineConfig({
  plugins: [
    react(),
    tailwindcss(),
  ],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
  },
  resolve: {
    alias: {
      "@": "/src",
    },
  },
  build: {
    outDir: "dist",
    emptyOutDir: true,
    chunkSizeWarningLimit: 600, // Tauri app loads locally; 525KB is fine (161KB gzipped)
    rollupOptions: {
      output: {
        manualChunks: {
          // Split syntax highlighter into its own chunk
          "syntax-highlighter": ["react-syntax-highlighter"],
        },
      },
    },
  },
});

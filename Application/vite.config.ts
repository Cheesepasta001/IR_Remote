import { defineConfig } from 'vite';

// Tauri drives this; the fixed port is what tauri.conf.json's devUrl points at.
export default defineConfig({
  clearScreen: false,
  server: {
    port: 5173,
    strictPort: true,
  },
  build: {
    outDir: 'dist',
    emptyOutDir: true,
    target: 'esnext',
    // Keep the bundle readable so `npm run verify:no-secrets` is meaningful.
    minify: false,
  },
});

import { svelte } from '@sveltejs/vite-plugin-svelte'
import { defineConfig } from 'vite'

// In dev, Vite serves the frontend and proxies /api to the Go server.
export default defineConfig({
  plugins: [svelte()],
  build: { chunkSizeWarningLimit: 1500 },
  // The reader is imported on demand; without this Vite only discovers it when a PDF is opened,
  // re-optimizes the dependencies and the open tab keeps the stale version (504 Outdated Optimize Dep).

  optimizeDeps: { include: ['pdfjs-dist', 'pdfjs-dist/web/pdf_viewer.mjs'] },
  server: {
    proxy: { '/api': 'http://127.0.0.1:8080' },
  },
})

import { svelte } from '@sveltejs/vite-plugin-svelte'
import { defineConfig } from 'vite'

// Em dev, o Vite serve o front e repassa /api para o servidor Go.
export default defineConfig({
  plugins: [svelte()],
  build: { chunkSizeWarningLimit: 1500 },
  // O leitor é importado sob demanda; sem isto o Vite só o descobre ao abrir um PDF,
  // reotimiza as dependências e a aba aberta fica com a versão antiga (504 Outdated Optimize Dep).
  optimizeDeps: { include: ['pdfjs-dist', 'pdfjs-dist/web/pdf_viewer.mjs'] },
  server: {
    proxy: { '/api': 'http://127.0.0.1:8080' },
  },
})

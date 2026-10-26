import { resolve } from 'node:path';
import { defineConfig } from 'vite';

// Three pages (docs/web-reproject.md): the landing page, the GML page and
// the reproject page, each loading only its own code.
// `base: './'`: GitHub Pages serves the page under /<repository>/.
export default defineConfig({
  base: './',
  worker: { format: 'es' },
  build: {
    target: 'es2022',
    chunkSizeWarningLimit: 2000,
    rollupOptions: {
      input: {
        landing: resolve(import.meta.dirname, 'index.html'),
        gml: resolve(import.meta.dirname, 'gml/index.html'),
        reproject: resolve(import.meta.dirname, 'reproject/index.html'),
      },
    },
  },
});

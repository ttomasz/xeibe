import { defineConfig } from 'vite';

// `base: './'`: GitHub Pages serves the page under /<repository>/.
export default defineConfig({
  base: './',
  worker: { format: 'es' },
  build: { target: 'es2022', chunkSizeWarningLimit: 2000 },
});

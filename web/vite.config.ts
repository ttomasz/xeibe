import { statSync } from 'node:fs';
import { resolve } from 'node:path';
import { defineConfig } from 'vite';

/** A wasm module's size, for the loading screen's progress
 * (src/common/loading.ts); 0 if it isn't built yet. */
function wasmSize(name: string): string {
  try {
    return String(statSync(resolve(import.meta.dirname, 'src/wasm', name)).size);
  } catch {
    return '0';
  }
}

// Three pages (docs/web-reproject.md): the landing page, the GML page and
// the reproject page, each loading only its own code.
// `base: './'`: GitHub Pages serves the page under /<repository>/.
export default defineConfig({
  base: './',
  define: {
    __XEIBE_WASM_SIZE__: wasmSize('xeibe_wasm_bg.wasm'),
    __XEIBE_WASM_REPROJECT_SIZE__: wasmSize('xeibe_wasm_reproject_bg.wasm'),
  },
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

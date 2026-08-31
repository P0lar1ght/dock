import { fileURLToPath } from 'node:url';
import { build } from 'vite';

const packageRoot = fileURLToPath(new URL('..', import.meta.url));
const embedEntry = fileURLToPath(new URL('../src/embed.ts', import.meta.url));

await build({
  root: packageRoot,
  configFile: false,
  build: {
    assetsInlineLimit: Number.MAX_SAFE_INTEGER,
    cssCodeSplit: false,
    emptyOutDir: false,
    sourcemap: true,
    lib: {
      entry: embedEntry,
      name: 'Dock',
      formats: ['iife'],
      fileName: () => 'dock-embed.js'
    },
    rollupOptions: {
      output: {
        inlineDynamicImports: true
      }
    }
  }
});

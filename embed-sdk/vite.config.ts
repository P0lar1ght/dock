import { fileURLToPath } from 'node:url';
import { defineConfig } from 'vite';

const fromPackageRoot = (path: string): string =>
  fileURLToPath(new URL(path, import.meta.url));

export default defineConfig({
  build: {
    emptyOutDir: false,
    sourcemap: true,
    lib: {
      entry: {
        index: fromPackageRoot('./src/index.ts'),
        client: fromPackageRoot('./src/client.ts'),
        embed: fromPackageRoot('./src/embed.ts')
      },
      formats: ['es']
    },
    rollupOptions: {
      output: {
        entryFileNames: '[name].js'
      }
    }
  }
});

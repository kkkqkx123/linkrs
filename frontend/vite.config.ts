import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import tailwindcss from '@tailwindcss/vite';
import monacoEditorPlugin from '@dvaji/vite-plugin-monaco-editor';
import path from 'path';

export default defineConfig({
  plugins: [
    svelte(),
    tailwindcss(),
    // Only the base editor worker is needed: Cypher highlighting is provided by
    // a Monarch tokenizer and completion items, none of which require a
    // language-specific web worker.
    monacoEditorPlugin({ languageWorkers: ['editorWorkerService'] }),
  ],
  resolve: {
    alias: {
      $lib: path.resolve('./src/lib'),
      $types: path.resolve('./src/lib/types'),
      $utils: path.resolve('./src/lib/utils'),
      $services: path.resolve('./src/lib/services'),
      $stores: path.resolve('./src/lib/stores'),
      $config: path.resolve('./src/lib/config'),
      $i18n: path.resolve('./src/lib/i18n/index.ts'),
      $components: path.resolve('./src/lib/components'),
      $pages: path.resolve('./src/lib/pages'),
    },
  },
  server: {
    port: 3011,
    proxy: {
      '/v1': {
        target: 'http://localhost:9758',
        changeOrigin: true,
      },
      '/api': {
        target: 'http://localhost:9758',
        changeOrigin: true,
      },
    },
  },
  build: {
    rollupOptions: {
      output: {
        manualChunks(id) {
          if (id.includes('node_modules')) {
            if (id.includes('monaco-editor')) return 'monaco-vendor';
            if (id.includes('cytoscape')) return 'cytoscape-vendor';
            if (id.includes('axios') || id.includes('lodash') || id.includes('dayjs') || id.includes('json-bigint')) return 'utils-vendor';
            return 'vendor';
          }
        },
      },
    },
    chunkSizeWarningLimit: 2000,
  },
});
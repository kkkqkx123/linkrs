import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import tailwindcss from '@tailwindcss/vite';
import monacoEditorPlugin from '@dvaji/vite-plugin-monaco-editor';
import path from 'path';

// Preview-only config: no backend proxy (all data comes from the mock layer),
// different port from the real frontend.
export default defineConfig({
  plugins: [
    svelte(),
    tailwindcss(),
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
    port: 3012,
  },
  define: {
    // Force mock mode at build time; the double-mode client still honors
    // VITE_USE_MOCK for parity with the real frontend.
    'import.meta.env.VITE_USE_MOCK': JSON.stringify('true'),
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

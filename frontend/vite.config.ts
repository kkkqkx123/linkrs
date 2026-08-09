import { defineConfig } from 'vite';
import { svelte } from '@sveltejs/vite-plugin-svelte';
import tailwindcss from '@tailwindcss/vite';
import path from 'path';

export default defineConfig({
  plugins: [svelte(), tailwindcss()],
  resolve: {
    alias: {
      $lib: path.resolve('./src/lib'),
      $types: path.resolve('./src/lib/types'),
      $utils: path.resolve('./src/lib/utils'),
      $services: path.resolve('./src/lib/services'),
      $stores: path.resolve('./src/lib/stores'),
      $config: path.resolve('./src/lib/config'),
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
            if (id.includes('cytoscape')) return 'cytoscape-vendor';
            if (id.includes('axios') || id.includes('lodash') || id.includes('dayjs') || id.includes('json-bigint')) return 'utils-vendor';
            return 'vendor';
          }
        },
      },
    },
    chunkSizeWarningLimit: 500,
  },
});
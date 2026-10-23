import { defineConfig } from 'vite';
import { sveltekit } from '@sveltejs/kit/vite';
import adapter from '@sveltejs/adapter-static';
import { paraglideVitePlugin } from '@inlang/paraglide-js';
import tailwindcss from '@tailwindcss/vite';
import path from 'path';
import { visualizer } from 'rollup-plugin-visualizer';
import { grammarWasmPlugin } from './vite/grammarWasm.ts';

export default defineConfig({
	plugins: [
		grammarWasmPlugin(),
		sveltekit({
			adapter: adapter({ fallback: 'index.html' }),
			files: { assets: 'public' },
		}),
		paraglideVitePlugin({
			project: './project.inlang',
			outdir: './src/lib/paraglide',
			emitTsDeclarations: true,
			// An explicit choice must win over the browser's language, and the app
			// is client-rendered so no server-visible strategy is needed.
			strategy: ['localStorage', 'preferredLanguage', 'baseLocale'],
			localStorageKey: 'linkrs_language',
		}),
		tailwindcss(),
		visualizer({
			template: 'markdown',
			emitFile: true,
			filename: 'bundle-report.md',
			gzipSize: true,
			brotliSize: true,
		}),
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
			$paraglide: path.resolve('./src/lib/paraglide'),
		},
	},
	server: {
		fs: {
			// The grammar crate holds the wasm module and the highlight query
			// that the editor imports, so the dev server has to read them.
			allow: ['../crates/linkrs-grammar'],
		},
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
		chunkSizeWarningLimit: 600,
	},
});

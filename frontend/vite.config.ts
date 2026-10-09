import { defineConfig } from 'vite';
import { sveltekit } from '@sveltejs/kit/vite';
import adapter from '@sveltejs/adapter-static';
import { paraglideVitePlugin } from '@inlang/paraglide-js';
import tailwindcss from '@tailwindcss/vite';
import path from 'path';

export default defineConfig({
	plugins: [
		sveltekit({ adapter: adapter({ fallback: 'index.html' }) }),
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
		chunkSizeWarningLimit: 4000,
	},
});

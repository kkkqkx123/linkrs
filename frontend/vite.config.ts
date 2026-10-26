import { defineConfig } from 'vite';
import { sveltekit } from '@sveltejs/kit/vite';
import adapter from '@sveltejs/adapter-static';
import { paraglideVitePlugin } from '@inlang/paraglide-js';
import tailwindcss from '@tailwindcss/vite';
import path from 'path';
import { visualizer } from 'rollup-plugin-visualizer';
import { grammarWasmPlugin } from './vite/grammarWasm.ts';

export default defineConfig(({ mode }) => {
	const isPreview = mode === 'preview' || mode === 'mock';
	const alias = [
		...(isPreview
			? [
					{
						find: '$lib/api/client',
						replacement: path.resolve('./src/lib/api/client.preview.ts'),
					},
				]
			: []),
		{ find: '$lib', replacement: path.resolve('./src/lib') },
		{ find: '$types', replacement: path.resolve('./src/lib/types') },
		{ find: '$utils', replacement: path.resolve('./src/lib/utils') },
		{ find: '$services', replacement: path.resolve('./src/lib/services') },
		{ find: '$stores', replacement: path.resolve('./src/lib/stores') },
		{ find: '$config', replacement: path.resolve('./src/lib/config') },
		{ find: '$i18n', replacement: path.resolve('./src/lib/i18n/index.ts') },
		{ find: '$components', replacement: path.resolve('./src/lib/components') },
		{ find: '$paraglide', replacement: path.resolve('./src/lib/paraglide') },
	];

	return {
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
			// The bundle report costs several seconds of build time; opt in with
			// VITE_BUNDLE_REPORT=1 when auditing chunk sizes.
			...(process.env.VITE_BUNDLE_REPORT
				? [
						visualizer({
							template: 'markdown',
							emitFile: true,
							filename: 'bundle-report.md',
							gzipSize: true,
							brotliSize: true,
						}),
					]
				: []),
		],
		resolve: {
			alias,
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
	};
});

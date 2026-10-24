import { defineConfig } from 'vite';
import { sveltekit } from '@sveltejs/kit/vite';
import adapter from '@sveltejs/adapter-static';
import { paraglideVitePlugin } from '@inlang/paraglide-js';
import tailwindcss from '@tailwindcss/vite';
import path from 'path';
import { grammarWasmPlugin } from './vite/grammarWasm.ts';

/**
 * Preview variant of the studio config: same plugins and aliases, but no
 * backend proxy (mock mode answers everything in-repo; `dev:real` talks to
 * API_BASE_URL directly) and a dedicated dev port.
 */
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
		fs: {
			// Same grammar crate as the main frontend, one level up from here.
			allow: ['../crates/linkrs-grammar'],
		},
		port: 3012,
		// No proxy: `dev` runs fully on mock; `dev:real` hits API_BASE_URL.
	},
	build: {
		chunkSizeWarningLimit: 600,
	},
});

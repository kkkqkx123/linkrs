import { defineEnvVars } from '@sveltejs/kit/env';

/**
 * Environment variables visible to the browser.
 *
 * `API_BASE_URL` points at the backend. Mock/preview mode is selected at build
 * time by the Vite `mode` (see vite.config.ts), which aliases the API client to
 * the in-repo mock implementation; it is not controlled by a runtime flag.
 */
export const variables = defineEnvVars({
	API_BASE_URL: {
		public: true,
		static: true,
		description: 'Backend base URL used when the mock layer is off',
		schema: (value) => value ?? 'http://localhost:9758',
	},
});

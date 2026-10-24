import { defineEnvVars } from '@sveltejs/kit/env';

/**
 * Environment variables visible to the browser.
 *
 * `USE_MOCK` routes API calls through the in-repo mock layer so the studio can
 * be exercised without a running backend; `API_BASE_URL` points at the backend
 * when the mock layer is off.
 */
export const variables = defineEnvVars({
	USE_MOCK: {
		public: true,
		static: true,
		description:
			'Serve API requests from the in-repo mock layer instead of the backend',
		schema: (value) => value === 'true',
	},
	API_BASE_URL: {
		public: true,
		static: true,
		description: 'Backend base URL used when the mock layer is off',
		schema: (value) => value ?? 'http://localhost:9758',
	},
});

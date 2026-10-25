/**
 * Thin entry for the API client.
 *
 * In preview mode Vite resolves `$lib/api/client` to `client.preview.ts`
 * via `resolve.alias`; in every other mode this module (re-exporting the
 * real implementation) is used. Shared code imports from here and stays
 * unaware of the mode split.
 */
export * from './client.real';

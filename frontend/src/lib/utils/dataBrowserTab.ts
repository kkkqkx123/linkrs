import type { DataBrowserTab } from '$types/dataBrowser';

/** Tabs addressable through the data-browser `tab` query parameter. */
const TABS: readonly DataBrowserTab[] = [
	'vertices',
	'edges',
	'vector',
	'fulltext',
];

/** Default tab when the query value is missing or unknown. */
export const DEFAULT_DATA_BROWSER_TAB: DataBrowserTab = 'vertices';

/**
 * Parse the `tab` query parameter into a data-browser tab. Unknown or
 * missing values fall back to the default so shared links never render
 * an empty page.
 */
export function parseDataBrowserTab(value: unknown): DataBrowserTab {
	if (
		typeof value === 'string' &&
		(TABS as readonly string[]).includes(value)
	) {
		return value as DataBrowserTab;
	}
	return DEFAULT_DATA_BROWSER_TAB;
}

/**
 * Extract the node id carried by a retrieval hit row. Vector and fulltext
 * searches project the document id under different column names, so the
 * first non-empty candidate wins; hits without any id cannot jump to
 * the graph.
 */
export function retrievalHitId(row: Record<string, unknown>): string {
	for (const key of ['id', 'doc_id', 'vid', 'vertex_id']) {
		const value = row[key];
		if (typeof value === 'string' && value.trim() !== '') return value;
		if (typeof value === 'number' && Number.isFinite(value))
			return String(value);
	}
	return '';
}

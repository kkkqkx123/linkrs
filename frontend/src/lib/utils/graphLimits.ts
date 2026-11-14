import type { LayoutType } from '$types/graph';

/** Maximum nodes kept on the interactive canvas per load or merge. */
export const MAX_GRAPH_NODES = 1000;

/** Maximum edges kept on the interactive canvas per load or merge. */
export const MAX_GRAPH_EDGES = 2000;

/** Element count above which force layout degrades to a cheaper strategy. */
export const LARGE_GRAPH_ELEMENTS = 400;

/** Canvas zoom below which node text labels hide to keep frames usable. */
export const LABEL_HIDE_ZOOM = 0.6;

export interface TruncatedCollection<T> {
	items: T[];
	total: number;
	truncated: boolean;
}

/**
 * Keep the first `limit` entries of a graph collection in query order,
 * recording the attempted total and whether anything was cut. Callers
 * display `items.length` of `total` with a truncation notice when set.
 */
export function truncateToLimit<T>(
	items: T[],
	limit: number,
): TruncatedCollection<T> {
	if (items.length <= limit)
		return { items, total: items.length, truncated: false };
	return {
		items: items.slice(0, limit),
		total: items.length,
		truncated: true,
	};
}

/** Large graphs default to simplified rendering until the user opts out. */
export function shouldSimplify(totalElements: number): boolean {
	return totalElements > LARGE_GRAPH_ELEMENTS;
}

/**
 * Pick the layout actually run on the canvas. Large force graphs degrade
 * to grid so interaction stays usable; explicit non-force choices are
 * always honored.
 */
export function resolveCanvasLayout(
	requested: LayoutType,
	elementCount: number,
): LayoutType {
	if (requested === 'force' && elementCount > LARGE_GRAPH_ELEMENTS)
		return 'grid';
	return requested;
}

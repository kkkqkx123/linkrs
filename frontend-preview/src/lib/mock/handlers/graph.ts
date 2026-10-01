/**
 * Mock handlers for graph browsing endpoints (`/api/v1/graph/**`).
 * All entries return the standard envelope because their callers use
 * `unwrap()`.
 */

import { envelope, type MockEntry, type MockHandler, type MockRegistry } from '../index';
import { scenarioIsEmpty } from '../scenario';
import { demoEdges, demoVertices, findVertex } from '../fixtures';

export const graphHandlers: MockRegistry = {
	'GET /api/v1/graph/vertices/{vid}': ((ctx) => {
		const vertex = findVertex(String(ctx.path.vid ?? ''));
		if (!vertex) {
			return envelope(null);
		}
		return envelope({
			vid: vertex.id,
			tags: { [vertex.tag]: vertex.properties }
		});
	}) satisfies MockEntry,

	'GET /api/v1/graph/vertices/{vid}/neighbors': ((ctx) => {
		const vid = String(ctx.path.vid ?? '');
		const direction = String(ctx.query.direction ?? 'BOTH');
		const neighbors = scenarioIsEmpty()
			? []
			: demoEdges
					.filter((e) => e.src === vid || e.dst === vid)
					.map((e) => {
						const other = findVertex(e.src === vid ? e.dst : e.src);
						return {
							vertex: { vid: other?.id ?? '', tag: other?.tag ?? 'person' },
							properties: other?.properties ?? {},
							edge_type: e.type,
							direction,
							rank: e.rank
						};
					});
		return envelope({ neighbors });
	}) satisfies MockEntry,

	'GET /api/v1/graph/edges': ((ctx) => {
		const src = ctx.query.src ? String(ctx.query.src) : undefined;
		const dst = ctx.query.dst ? String(ctx.query.dst) : undefined;
		const edges = scenarioIsEmpty()
			? []
			: demoEdges.filter((e) => (!src || e.src === src) && (!dst || e.dst === dst));
		return envelope({
			edges: edges.map((e) => ({
				src: e.src,
				dst: e.dst,
				edge_type: e.type,
				rank: e.rank,
				properties: e.properties
			}))
		});
	}) satisfies MockEntry
};

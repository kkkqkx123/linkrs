/**
 * Mock handlers for the data browser endpoints
 * (`/api/v1/data/spaces/{name}/...`). Entries return the standard envelope
 * because their callers use `unwrap()`.
 */

import { envelope, type MockEntry, type MockRegistry } from '../index';
import { scenarioIsEmpty } from '../scenario';
import { demoEdges, demoSpace, demoVertices } from '../fixtures';

function paginate<T>(
	items: T[],
	page: number,
	pageSize: number,
): { data: T[]; total: number } {
	const total = items.length;
	const start = (page - 1) * pageSize;
	return { data: items.slice(start, start + pageSize), total };
}

export const dataBrowserHandlers: MockRegistry = {
	'GET /api/v1/data/spaces/{name}/tags/{tag_name}/vertices': ((ctx) => {
		const page = Number(ctx.query.page ?? 1) || 1;
		const pageSize = Number(ctx.query.page_size ?? 50) || 50;
		const tag = String(ctx.path.tag_name ?? '');
		const items = scenarioIsEmpty()
			? []
			: demoVertices
					.filter((v) => v.tag === tag)
					.map((v) => ({ id: v.id, tag: v.tag, properties: v.properties }));
		const { data, total } = paginate(items, page, pageSize);
		return envelope({ data, total, page, pageSize });
	}) satisfies MockEntry,

	'GET /api/v1/data/spaces/{name}/edge-types/{edge_name}/edges': ((ctx) => {
		const page = Number(ctx.query.page ?? 1) || 1;
		const pageSize = Number(ctx.query.page_size ?? 50) || 50;
		const edgeType = String(ctx.path.edge_name ?? '');
		const items = scenarioIsEmpty()
			? []
			: demoEdges
					.filter((e) => e.type === edgeType)
					.map((e) => ({
						id: e.id,
						type: e.type,
						src: e.src,
						dst: e.dst,
						rank: e.rank,
						properties: e.properties,
					}));
		const { data, total } = paginate(items, page, pageSize);
		return envelope({ data, total, page, pageSize });
	}) satisfies MockEntry,

	'GET /api/v1/schema/spaces/{name}/statistics': ((ctx) => {
		const name = String(ctx.path.name ?? demoSpace);
		return envelope({
			space: name,
			totalVertices: scenarioIsEmpty() ? 0 : demoVertices.length,
			totalEdges: scenarioIsEmpty() ? 0 : demoEdges.length,
			tagCount: 2,
			edgeTypeCount: 2,
			tagDistribution: [
				{ tag: 'person', count: 3 },
				{ tag: 'company', count: 1 },
			],
			edgeTypeDistribution: [
				{ type: 'knows', count: 2 },
				{ type: 'works_at', count: 1 },
			],
		});
	}) satisfies MockEntry,
};

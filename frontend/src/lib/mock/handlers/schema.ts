/**
 * Mock handlers for schema management endpoints (`/v1/schema/**`,
 * `/api/v1/schema/**`). `/api/v1/**` entries return the standard envelope
 * because their callers use `unwrap()`.
 */

import { envelope, type MockEntry, type MockRegistry } from '../index';
import { scenarioIsEmpty } from '../scenario';
import { demoEdgeTypes, demoSpace, demoTags, demoVertices } from '../fixtures';

const spaces = [{ id: 1, name: demoSpace, vid_type: 'STRING' }];

function listOrEmpty<T>(items: T[]): T[] {
	return scenarioIsEmpty() ? [] : items;
}

export const schemaHandlers: MockRegistry = {
	'GET /v1/schema/spaces': { spaces: listOrEmpty(spaces) } satisfies MockEntry,

	'POST /v1/schema/spaces': ((ctx) => {
		const body = (ctx.body ?? {}) as { name?: unknown };
		return {
			message: 'space created',
			space_name: String(body.name ?? demoSpace),
		};
	}) satisfies MockEntry,

	'GET /v1/schema/spaces/{name}': ((ctx) => ({
		space: { id: 1, name: String(ctx.path.name ?? demoSpace) },
	})) satisfies MockEntry,

	'DELETE /v1/schema/spaces/{name}': ((ctx) => ({
		message: 'space dropped',
		space_name: String(ctx.path.name ?? demoSpace),
	})) satisfies MockEntry,

	'GET /v1/schema/spaces/{name}/tags': demoTags satisfies MockEntry,

	'POST /v1/schema/spaces/{name}/tags': ((ctx) => {
		const body = (ctx.body ?? {}) as { name?: unknown };
		return { message: 'tag created', tag_name: String(body.name ?? 'person') };
	}) satisfies MockEntry,

	'GET /api/v1/schema/spaces/{name}/tags/{tag_name}': envelope(
		demoTags[0],
	) satisfies MockEntry,

	'DELETE /api/v1/schema/spaces/{name}/tags/{tag_name}': {
		message: 'tag dropped',
	} satisfies MockEntry,

	'GET /v1/schema/spaces/{name}/edge-types': demoEdgeTypes satisfies MockEntry,

	'POST /v1/schema/spaces/{name}/edge-types': ((ctx) => {
		const body = (ctx.body ?? {}) as { name?: unknown };
		return {
			message: 'edge type created',
			edge_type_name: String(body.name ?? 'knows'),
		};
	}) satisfies MockEntry,

	'GET /api/v1/schema/spaces/{name}/edge-types/{edge_name}': envelope(
		demoEdgeTypes[0],
	) satisfies MockEntry,

	'DELETE /api/v1/schema/spaces/{name}/edge-types/{edge_name}': {
		message: 'edge type dropped',
	} satisfies MockEntry,

	'GET /api/v1/schema/spaces/{name}/details': envelope({
		name: demoSpace,
		vid_type: 'STRING',
		tags: listOrEmpty(demoTags),
		edge_types: listOrEmpty(demoEdgeTypes),
		comment: 'demo space',
	}) satisfies MockEntry,

	'GET /api/v1/schema/spaces/{name}/statistics': envelope({
		vertex_count: demoVertices.length,
		edge_count: 3,
		tag_count: demoTags.length,
		edge_type_count: demoEdgeTypes.length,
		tag_distribution: [
			{ tag: 'person', count: 3 },
			{ tag: 'company', count: 1 },
		],
		edge_type_distribution: [
			{ type: 'knows', count: 2 },
			{ type: 'works_at', count: 1 },
		],
	}) satisfies MockEntry,

	'GET /api/v1/schema/spaces/{name}/indexes': envelope([
		{
			id: 1,
			name: 'person_name_idx',
			type: 'TAG',
			schemaName: 'person',
			properties: ['name'],
			status: 'finished',
			created_at: 1735689800,
			progress: 100,
		},
		{
			id: 2,
			name: 'person_age_idx',
			type: 'TAG',
			schemaName: 'person',
			properties: ['age'],
			status: 'creating',
			created_at: 1735689900,
			progress: 45,
		},
	]) satisfies MockEntry,

	'POST /api/v1/schema/spaces/{name}/indexes': ((ctx) => {
		const body = (ctx.body ?? {}) as { name?: unknown };
		return { message: 'index created', index_name: String(body.name ?? 'idx') };
	}) satisfies MockEntry,

	'GET /api/v1/schema/spaces/{name}/indexes/{index_name}': envelope({
		id: 1,
		name: 'person_name_idx',
		type: 'TAG',
		schemaName: 'person',
		properties: ['name'],
		status: 'finished',
		created_at: 1735689800,
		progress: 100,
	}) satisfies MockEntry,

	'DELETE /api/v1/schema/spaces/{name}/indexes/{index_name}': {
		message: 'index dropped',
	} satisfies MockEntry,

	'POST /api/v1/schema/spaces/{name}/indexes/{index_name}/rebuild': {
		message: 'rebuild started',
		index_name: 'person_name_idx',
	} satisfies MockEntry,
};

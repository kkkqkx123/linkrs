/**
 * Mock handlers for query execution and cursor endpoints (`/v1/query**`).
 * Responses mirror the wire contract: `QueryResponse` for statements,
 * cursor open/fetch/close for paged reads.
 */

import { MockFailure, type MockEntry, type MockRegistry } from '../index';
import { scenarioIsEmpty } from '../scenario';
import { demoVertices } from '../fixtures';

const SELECT_COLUMNS = ['vid', 'name', 'age'];

function resultRows(): Record<string, unknown>[] {
	if (scenarioIsEmpty()) return [];
	return demoVertices.map((v) => ({
		vid: v.id,
		name: v.properties.name ?? v.properties.title ?? v.tag,
		age: v.properties.age ?? null,
		tag: v.tag,
	}));
}

function queryResponse(query: string): unknown {
	const trimmed = query.trim().toUpperCase();
	if (trimmed.startsWith('SHOW USERS')) {
		return {
			success: true,
			data: {
				columns: ['username', 'role', 'status'],
				rows: [
					{ username: 'root', role: 'GOD', status: 'enabled' },
					{ username: 'demo', role: 'ADMIN', status: 'enabled' },
				],
				row_count: 2,
			},
			metadata: {
				execution_time_ms: 3,
				result_row_count: 2,
				plan_node_count: 1,
			},
		};
	}
	if (trimmed.startsWith('CREATE USER') || trimmed.startsWith('ALTER USER')) {
		return {
			success: true,
			data: { columns: [], rows: [], row_count: 0 },
			metadata: {
				execution_time_ms: 3,
				result_row_count: 0,
				plan_node_count: 1,
			},
		};
	}
	if (trimmed.startsWith('SEARCH VECTOR') || trimmed.startsWith('SEARCH INDEX')) {
		const rows = scenarioIsEmpty()
			? []
			: [
					{ id: demoVertices[0].id, score: 0.98, payload: 'mock result 1' },
					{ id: demoVertices[1].id, score: 0.87, payload: 'mock result 2' },
				];
		return {
			success: true,
			data: { columns: ['id', 'score', 'payload'], rows, row_count: rows.length },
			metadata: { execution_time_ms: 8, result_row_count: rows.length, plan_node_count: 4 },
		};
	}
	if (
		trimmed.startsWith('SELECT') ||
		trimmed.startsWith('MATCH') ||
		trimmed.startsWith('FETCH')
	) {
		const rows = resultRows();
		return {
			success: true,
			data: { columns: SELECT_COLUMNS, rows, row_count: rows.length },
			metadata: {
				execution_time_ms: 12,
				result_row_count: rows.length,
				plan_node_count: 6,
			},
		};
	}
	if (trimmed.startsWith('INVALID') || trimmed.startsWith('ERROR')) {
		return {
			success: false,
			error: {
				code: 'PARSE_ERROR',
				message: 'Mock fixture: unparsable statement',
			},
		};
	}
	return {
		success: true,
		data: { columns: [], rows: [], row_count: 0 },
		metadata: { execution_time_ms: 3, result_row_count: 0, plan_node_count: 2 },
	};
}

export const queryHandlers: MockRegistry = {
	'POST /v1/query': ((ctx) => {
		const body = (ctx.body ?? {}) as { query?: unknown };
		return queryResponse(typeof body.query === 'string' ? body.query : '');
	}) satisfies MockEntry,

	'POST /v1/query/batch': ((ctx) => {
		const body = (ctx.body ?? {}) as { statements?: unknown };
		const statements = Array.isArray(body.statements) ? body.statements : [];
		return {
			results: statements.map((q) =>
				queryResponse(typeof q === 'string' ? q : String((q as { query?: string }).query ?? '')),
			),
		};
	}) satisfies MockEntry,

	'POST /v1/query/validate': {
		valid: true,
		message: 'Mock fixture: valid query',
		estimated_rows: null,
	} satisfies MockEntry,

	'POST /v1/query/explain': ((ctx) => {
		const body = (ctx.body ?? {}) as { query?: unknown };
		const query = typeof body.query === 'string' ? body.query : '';
		const base = queryResponse(query) as Record<string, unknown>;
		return {
			...base,
			metadata: { ...(base.metadata as Record<string, unknown>), plan_node_count: 8 },
			plan: { nodes: 8, edges: 3, estimated_rows: 4 },
		};
	}) satisfies MockEntry,

	'POST /v1/query/cursor/open': ((ctx) => {
		const body = (ctx.body ?? {}) as { query?: unknown };
		const query = typeof body.query === 'string' ? body.query : '';
		if (!query.trim()) {
			throw new MockFailure(
				400,
				'EMPTY_QUERY',
				'Mock fixture: empty cursor query',
			);
		}
		return { cursor_id: 900001, columns: SELECT_COLUMNS };
	}) satisfies MockEntry,

	'POST /v1/query/cursor/fetch': ((ctx) => {
		const body = (ctx.body ?? {}) as {
			page_size?: unknown;
			cursor_id?: unknown;
		};
		const pageSize = typeof body.page_size === 'number' ? body.page_size : 100;
		const rows = resultRows();
		// First fetch page (cursor_id 900001) returns everything; later pages empty.
		const isFirstPage = body.cursor_id === 900001;
		const page = isFirstPage ? rows.slice(0, pageSize) : [];
		return {
			columns: SELECT_COLUMNS,
			rows: page,
			has_more: false,
			returned: page.length,
		};
	}) satisfies MockEntry,

	'POST /v1/query/cursor/close': { success: true } satisfies MockEntry,
};

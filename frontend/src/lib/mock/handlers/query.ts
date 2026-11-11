/**
 * Mock handlers for query execution and cursor endpoints (`/v1/query**`).
 * Responses mirror the wire contract: `QueryResponse` for statements,
 * cursor open/fetch/close for paged reads.
 */

import { MockFailure, type MockEntry, type MockRegistry } from '../index';
import { scenarioIsEmpty } from '../scenario';
import { demoVertices } from '../fixtures';

const SELECT_COLUMNS = ['vid', 'name', 'age'];

// Plan fixture shaped like the backend table renderer: a single `plan`
// column whose value is the whole ASCII plan, with a deps chain the workbench
// can turn into a collapsible operator tree.
const MOCK_PLAN_TABLE = [
	'Execution Model: Batch Pull',
	'Parallel Workers: requested=1',
	'',
	'+------+------------------+------------+--------------------------------------------------+--------------------------------------------------+------------------+',
	'| id   | name             | deps       | profiling_data                                    | operator_info                                    | output_var       |',
	'+------+------------------+------------+--------------------------------------------------+--------------------------------------------------+------------------+',
	'|    0 | ResultSink       | 1          | -                                                | est_rows:2                                       | __Value_2        |',
	'+------+------------------+------------+--------------------------------------------------+--------------------------------------------------+------------------+',
	'|    1 | Filter           | 2          | -                                                | predicate:(n.age > 30),est_rows:12               | __Filter_1       |',
	'+------+------------------+------------+--------------------------------------------------+--------------------------------------------------+------------------+',
	'|    2 | ScanVertices     | -          | -                                                | table:Person,est_rows:100                        | __Value_0        |',
	'+------+------------------+------------+--------------------------------------------------+--------------------------------------------------+------------------+',
].join('\n');

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
		if (query.trim().toUpperCase().startsWith('INVALID')) {
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
			data: {
				columns: ['plan'],
				rows: [{ plan: MOCK_PLAN_TABLE }],
				row_count: 1,
			},
			metadata: {
				execution_time_ms: 4,
				result_row_count: 1,
				plan_node_count: 3,
				stages: {
					parse_ms: 0.25,
					validate_ms: 0.1,
					plan_ms: 1.2,
					optimize_ms: 0.8,
					execute_ms: 1.5,
				},
			},
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

	'GET /v1/queries': { queries: [] } satisfies MockEntry,

	'POST /v1/sessions/{id}/queries/{qid}/kill': ((ctx) => ({
		session_id: Number(ctx.path.id ?? 0),
		query_id: Number(ctx.path.qid ?? 0),
		status: 'killed',
	})) satisfies MockEntry,
};

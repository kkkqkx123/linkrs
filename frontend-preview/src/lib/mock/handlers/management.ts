/**
 * Mock handlers for management endpoints without dedicated fixtures yet:
 * extended sync/outbox, config item, batch jobs, import status,
 * migration, vector, fulltext, functions, schema versions and cursors.
 * Payloads mirror the server response shapes so pages can render.
 */

import type { MockEntry, MockRegistry } from '../index';

const now = '2025-01-01T00:00:00Z';

export const managementHandlers: MockRegistry = {
	'GET /v1/sync/outbox/diagnostics': {
		frontier_lag: 0,
		degraded: 0,
		dead_letters: 0,
	} satisfies MockEntry,

	'GET /v1/sync/outbox/dead_letters': { dead_letters: [] } satisfies MockEntry,

	'POST /v1/sync/outbox/requeue': { requeued: 0 } satisfies MockEntry,

	'POST /v1/sync/outbox/retry': { delivered: 0 } satisfies MockEntry,

	'GET /v1/sync/outbox/degraded_ranges': {
		degraded_ranges: [],
	} satisfies MockEntry,

	'POST /v1/sync/outbox/degraded/clear': { cleared: true } satisfies MockEntry,

	'POST /v1/sync/outbox/retention/run': {
		pruned: 0,
		archived: 0,
		retention_lsn: 0,
	} satisfies MockEntry,

	'GET /v1/sync/outbox/retention/status': { retention_lsn: 0 } satisfies MockEntry,

	'GET /v1/config/{section}/{key}': ((ctx) => ({
		section: String(ctx.path.section ?? ''),
		key: String(ctx.path.key ?? ''),
		value: null,
	})) satisfies MockEntry,

	'PUT /v1/config/{section}/{key}': { success: true } satisfies MockEntry,

	'DELETE /v1/config/{section}/{key}': { success: true } satisfies MockEntry,

	'POST /v1/batch': {
		batch_id: 'mock-batch-1',
		status: 'created',
		created_at: now,
	} satisfies MockEntry,

	'GET /v1/batch/{id}': ((ctx) => ({
		batch_id: String(ctx.path.id ?? 'mock-batch-1'),
		status: 'completed',
		progress: {
			total: 0,
			processed: 0,
			succeeded: 0,
			failed: 0,
			buffered: 0,
		},
		created_at: now,
		updated_at: now,
	})) satisfies MockEntry,

	'POST /v1/batch/{id}/items': {
		accepted: 0,
		buffered: 0,
		total_buffered: 0,
	} satisfies MockEntry,

	'POST /v1/batch/{id}/execute': ((ctx) => ({
		batch_id: String(ctx.path.id ?? 'mock-batch-1'),
		status: 'completed',
		result: {
			vertices_inserted: 0,
			edges_inserted: 0,
			vertices_updated: 0,
			edges_updated: 0,
			vertices_deleted: 0,
			edges_deleted: 0,
			errors: [],
		},
		completed_at: now,
	})) satisfies MockEntry,

	'POST /v1/batch/{id}/cancel': { success: true } satisfies MockEntry,

	'DELETE /v1/batch/{id}': { success: true } satisfies MockEntry,

	'GET /v1/import/{id}': ((ctx) => ({
		job_id: String(ctx.path.id ?? 'mock-import-1'),
		status: 'completed',
		rows_imported: 0,
		rows_failed: 0,
	})) satisfies MockEntry,

	'POST /v1/migration/plan/{space}/{label}': {
		plan_json: '{}',
		safety_level: 'safe',
		estimated_rows: 0,
		steps: [],
	} satisfies MockEntry,

	'POST /v1/migration/execute': {
		success: true,
		steps_completed: 0,
		rows_migrated: 0,
		errors: [],
	} satisfies MockEntry,

	'POST /v1/migration/rollback': {
		success: true,
		steps_completed: 0,
		rows_migrated: 0,
		errors: [],
	} satisfies MockEntry,

	'POST /v1/migration/dry-run': {
		success: true,
		steps_completed: 0,
		rows_migrated: 0,
		errors: [],
	} satisfies MockEntry,

	'GET /v1/migration/history/{space}/{label}': { history: [] } satisfies MockEntry,

	'GET /v1/migration/status/{space}/{label}': { status: 'idle' } satisfies MockEntry,

	'GET /v1/vector/indexes': { indexes: [] } satisfies MockEntry,

	'GET /v1/vector/indexes/{space_id}/{tag_name}/{field_name}': {
		exists: false,
	} satisfies MockEntry,

	'POST /v1/vector/indexes': { success: true } satisfies MockEntry,

	'DELETE /v1/vector/indexes/{space_id}/{tag_name}/{field_name}': {
		success: true,
	} satisfies MockEntry,

	'POST /v1/vector/search': { results: [] } satisfies MockEntry,

	'POST /v1/vector/indexes/rebuild': {
		rebuild_id: 'mock-vector-rebuild-1',
		status: 'completed',
	} satisfies MockEntry,

	'GET /v1/vector/rebuilds/{id}': ((ctx) => ({
		rebuild_id: String(ctx.path.id ?? 'mock-vector-rebuild-1'),
		status: 'completed',
	})) satisfies MockEntry,

	'POST /v1/vector/indexes/clear': { ok: true } satisfies MockEntry,

	'GET /v1/fulltext/indexes/inconsistent': { indexes: [] } satisfies MockEntry,

	'POST /v1/fulltext/indexes/rebuild': {
		rebuild_id: 'mock-fulltext-rebuild-1',
		status: 'completed',
	} satisfies MockEntry,

	'GET /v1/fulltext/rebuilds/{id}': ((ctx) => ({
		rebuild_id: String(ctx.path.id ?? 'mock-fulltext-rebuild-1'),
		status: 'completed',
	})) satisfies MockEntry,

	'POST /v1/fulltext/indexes/clear': { ok: true } satisfies MockEntry,

	'GET /v1/functions': { functions: [] } satisfies MockEntry,

	'GET /v1/functions/{name}': ((ctx) => ({
		name: String(ctx.path.name ?? ''),
		function_type: 'custom',
		parameters: [],
		return_type: '',
		description: '',
	})) satisfies MockEntry,

	'POST /v1/functions': ((ctx) => {
		const body = (ctx.body ?? {}) as { name?: unknown };
		return { success: true, function_id: String(body.name ?? 'mock-fn') };
	}) satisfies MockEntry,

	'DELETE /v1/functions/{name}': { success: true } satisfies MockEntry,

	'GET /v1/schema/versions/{space}/{label}': { versions: [] } satisfies MockEntry,

	'GET /v1/schema/changes/{space}/{label}/{from_version}/{to_version}': {
		changes: [],
	} satisfies MockEntry,

	'GET /v1/schema/breaking-changes/{space}/{label}/{from_version}/{to_version}':
		{
			has_breaking_changes: false,
			changes: [],
			recommendation: '',
		} satisfies MockEntry,

	'POST /v1/query/cursor/open': {
		cursor_id: 9001,
		columns: ['n'],
	} satisfies MockEntry,

	'POST /v1/query/cursor/fetch': {
		columns: ['n'],
		rows: [],
		has_more: false,
		returned: 0,
	} satisfies MockEntry,

	'POST /v1/query/cursor/close': { success: true, closed: true } satisfies MockEntry,
};

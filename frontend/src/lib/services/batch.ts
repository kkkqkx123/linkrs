import { call, client } from '$lib/api/client';
import type { components } from '$lib/api/schema';

export type BatchType = components['schemas']['BatchType'];
export type BatchStatus = components['schemas']['BatchStatus'];
export type BatchItem = components['schemas']['BatchItem'];
export type BatchStatusResponse =
	components['schemas']['BatchStatusResponse'];
export type CreateBatchResponse =
	components['schemas']['CreateBatchResponse'];
export type AddBatchItemsResponse =
	components['schemas']['AddBatchItemsResponse'];
export type ExecuteBatchResponse =
	components['schemas']['ExecuteBatchResponse'];
export type BatchResultData = components['schemas']['BatchResultData'];
export type BatchProgress = components['schemas']['BatchProgress'];

export type BatchStatusLike = BatchStatus | string | null | undefined;

function normalizeStatus(status: BatchStatusLike): BatchStatus | null {
	if (
		status === 'created' ||
		status === 'running' ||
		status === 'completed' ||
		status === 'failed' ||
		status === 'cancelled'
	)
		return status;
	return null;
}

/** Items can only be appended before the first execution. */
export function canAddItems(status: BatchStatusLike): boolean {
	if (status === null || status === undefined) return true;
	return normalizeStatus(status) === 'created';
}

/** Execution starts from a freshly buffered task. */
export function canExecute(status: BatchStatusLike): boolean {
	if (status === null || status === undefined) return true;
	return normalizeStatus(status) === 'created';
}

/** Cancellation races a live task; finished tasks are no-ops server-side. */
export function canCancel(status: BatchStatusLike): boolean {
	if (status === null || status === undefined) return true;
	const current = normalizeStatus(status);
	return current === 'created' || current === 'running';
}

/** A running task owns its outcome until it settles, so removal waits. */
export function canRemove(status: BatchStatusLike): boolean {
	if (status === null || status === undefined) return true;
	return normalizeStatus(status) !== 'running';
}

/** Terminal states stop status polling. */
export function isTerminalStatus(status: BatchStatusLike): boolean {
	const current = normalizeStatus(status);
	return (
		current === 'completed' || current === 'failed' || current === 'cancelled'
	);
}

const BATCH_ITEM_TYPES = new Set([
	'vertex',
	'edge',
	'update_vertex',
	'update_edge',
	'delete_vertex',
	'delete_edge',
]);

export type BatchItemsParseError = 'array' | 'shape' | null;

function isRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === 'object' && value !== null && !Array.isArray(value);
}

/**
 * Parse pasted or uploaded text into batch items without throwing.
 * The caller maps the error code to a localized message so this module
 * stays independent of the active locale.
 */
export function parseBatchItems(text: string): {
	items: BatchItem[];
	error: BatchItemsParseError;
} {
	let parsed: unknown;
	try {
		parsed = JSON.parse(text || '[]');
	} catch {
		return { items: [], error: 'array' };
	}
	if (!Array.isArray(parsed)) return { items: [], error: 'array' };
	for (const entry of parsed) {
		if (!isRecord(entry)) return { items: [], error: 'shape' };
		if (
			typeof entry.type !== 'string' ||
			!BATCH_ITEM_TYPES.has(entry.type)
		)
			return { items: [], error: 'shape' };
	}
	return { items: parsed as BatchItem[], error: null };
}

/** Split a large upload into sequential add-items requests. */
export function chunkItems<T>(items: T[], size: number): T[][] {
	const step = Number.isFinite(size) && size > 0 ? Math.floor(size) : 500;
	const chunks: T[][] = [];
	for (let start = 0; start < items.length; start += step)
		chunks.push(items.slice(start, start + step));
	return chunks;
}

/**
 * Async batch-job management (`POST/GET /v1/batch*`). This is the job
 * API for bulk vertex/edge loads, distinct from the inline
 * `POST /v1/query/batch` multi-statement window used by the console and
 * from the file import/export endpoints used by the transfer page.
 */
export const batchService = {
	create: async (params: {
		space_id: number;
		batch_type: BatchType;
		batch_size?: number;
	}): Promise<CreateBatchResponse> =>
		call(
			client.POST('/v1/batch', {
				body: {
					space_id: params.space_id,
					batch_type: params.batch_type,
					batch_size: params.batch_size ?? 1000,
				},
			}),
		),

	status: async (id: string): Promise<BatchStatusResponse> =>
		call(
			client.GET('/v1/batch/{id}', {
				params: { path: { id } },
			}),
		),

	addItems: async (
		id: string,
		items: BatchItem[],
	): Promise<AddBatchItemsResponse> =>
		call(
			client.POST('/v1/batch/{id}/items', {
				params: { path: { id } },
				body: { items },
			}),
		),

	execute: async (id: string): Promise<ExecuteBatchResponse> =>
		call(
			client.POST('/v1/batch/{id}/execute', {
				params: { path: { id } },
			}),
		),

	cancel: async (id: string): Promise<unknown> =>
		call(
			client.POST('/v1/batch/{id}/cancel', {
				params: { path: { id } },
			}),
		),

	remove: async (id: string): Promise<unknown> =>
		call(
			client.DELETE('/v1/batch/{id}', {
				params: { path: { id } },
			}),
		),
};

export default batchService;

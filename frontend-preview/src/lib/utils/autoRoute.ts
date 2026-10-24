/** Automatic materialized/streaming routing for single statements. */

export type AutoPath = 'stream' | 'materialized';

export type ExecutionPreference = 'materialized' | 'stream' | 'auto';

export const AUTO_STREAM_THRESHOLD_MIN = 1;
export const AUTO_STREAM_THRESHOLD_MAX = 10_000_000;
export const DEFAULT_AUTO_STREAM_THRESHOLD = 1000;

export function clampAutoThreshold(value: number): number {
	if (!Number.isFinite(value)) return DEFAULT_AUTO_STREAM_THRESHOLD;
	return Math.min(
		AUTO_STREAM_THRESHOLD_MAX,
		Math.max(AUTO_STREAM_THRESHOLD_MIN, Math.floor(value)),
	);
}

export interface AutoRouteInput {
	/** Stream shape from eligibility: multi-statement scripts have no
	 * per-statement estimate yet, anything else routes on the estimate. */
	mode: 'single' | 'batch' | null;
	estimatedRows: number | null | undefined;
}

/**
 * Pick an execution path. Batch scripts always stream (the safe direction
 * for an unknown total), a missing estimate streams for the same reason,
 * and an estimate at or under the threshold stays materialized so small
 * queries render in one step. Empty or command-shaped input stays
 * materialized, matching the caller fallback.
 */
export function resolveAutoPath(
	input: AutoRouteInput,
	threshold: number,
): AutoPath {
	if (input.mode === 'batch') return 'stream';
	if (input.mode !== 'single') return 'materialized';
	const estimated = input.estimatedRows;
	if (estimated === null || estimated === undefined) return 'stream';
	return estimated > threshold ? 'stream' : 'materialized';
}

/**
 * Time-range windowing for monitoring trend series.
 *
 * The backend ships a fixed-length timeseries window in each overview
 * snapshot; preset ranges slice that window client-side so operators can
 * focus on the last N minutes without extra round-trips. The `live`
 * range keeps every point as the stream keeps appending fresh ones.
 */

export const TREND_RANGES = ['live', '5m', '15m', '1h'] as const;

export type TrendRange = (typeof TREND_RANGES)[number];

/** Duration in seconds of each preset range. */
export const RANGE_SECONDS: Record<Exclude<TrendRange, 'live'>, number> = {
	'5m': 300,
	'15m': 900,
	'1h': 3600,
};

/** Structural input: any point series carrying a unix-second stamp. */
export interface TimestampedPoint {
	second: number | string;
}

/**
 * Slice points to the requested range.
 *
 * `second` may arrive as a string when the JSON parser stringifies large
 * integers, so it is coerced before comparison.
 */
export function sliceTrendWindow<T extends TimestampedPoint>(
	points: T[],
	range: TrendRange,
	nowSeconds: number = Math.floor(Date.now() / 1000),
): T[] {
	if (range === 'live') return points;
	const cutoff = nowSeconds - RANGE_SECONDS[range];
	return points.filter((point) => Number(point.second) >= cutoff);
}

/** Millisecond `from`/`to` bounds for stats queries under a preset range. */
export function rangeQueryBounds(
	range: TrendRange,
	nowMs: number = Date.now(),
): { from: string; to: string } | undefined {
	if (range === 'live') return undefined;
	const seconds = RANGE_SECONDS[range];
	return {
		from: new Date(nowMs - seconds * 1000).toISOString(),
		to: new Date(nowMs).toISOString(),
	};
}

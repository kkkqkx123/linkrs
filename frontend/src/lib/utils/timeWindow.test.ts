import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import {
	RANGE_SECONDS,
	rangeQueryBounds,
	sliceTrendWindow,
} from './timeWindow.ts';

const NOW = 1_000_000;

function series(seconds: number[]) {
	return seconds.map((second) => ({ second, value: second * 2 }));
}

describe('sliceTrendWindow', () => {
	it('keeps all points in live mode', () => {
		const points = series([NOW - 9999, NOW - 60, NOW]);
		assert.equal(sliceTrendWindow(points, 'live', NOW).length, 3);
	});

	it('drops points older than the preset cutoff', () => {
		const points = series([NOW - 400, NOW - 100, NOW]);
		const sliced = sliceTrendWindow(points, '5m', NOW);
		assert.deepEqual(
			sliced.map((p) => p.second),
			[NOW - 100, NOW],
		);
	});

	it('coerces string seconds from big-int-tolerant JSON', () => {
		const points = [
			{ second: String(NOW - 700), value: 1 },
			{ second: String(NOW - 10), value: 2 },
		];
		const sliced = sliceTrendWindow(points, '5m', NOW);
		assert.deepEqual(
			sliced.map((p) => p.value),
			[2],
		);
	});

	it('returns an empty slice when all points are stale', () => {
		assert.deepEqual(sliceTrendWindow(series([NOW - 9999]), '15m', NOW), []);
	});
});

describe('rangeQueryBounds', () => {
	it('returns undefined for the live range', () => {
		assert.equal(rangeQueryBounds('live', NOW), undefined);
	});

	it('computes ISO bounds around the given instant', () => {
		const bounds = rangeQueryBounds('1h', NOW * 1000);
		assert.equal(bounds?.to, new Date(NOW * 1000).toISOString());
		assert.equal(
			bounds?.from,
			new Date(NOW * 1000 - RANGE_SECONDS['1h'] * 1000).toISOString(),
		);
	});
});

import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import {
	computeWindow,
	estimateColumnWidths,
	STREAM_MAX_COLUMN_WIDTH,
	STREAM_MIN_COLUMN_WIDTH,
} from './virtualWindow.ts';

const identity = (value: unknown): string => String(value ?? '');

describe('computeWindow', () => {
	it('renders the first screen with one screen of overscan below', () => {
		const window = computeWindow(100, 0, 320, 32);
		assert.deepEqual(window, {
			start: 0,
			end: 20,
			topSpacer: 0,
			bottomSpacer: 2560,
		});
	});

	it('clamps the tail screen without negative spacers', () => {
		const window = computeWindow(100, 2880, 320, 32);
		assert.deepEqual(window, {
			start: 80,
			end: 100,
			topSpacer: 2560,
			bottomSpacer: 0,
		});
	});

	it('returns an empty window for an empty buffer', () => {
		assert.deepEqual(computeWindow(0, 0, 320, 32), {
			start: 0,
			end: 0,
			topSpacer: 0,
			bottomSpacer: 0,
		});
	});

	it('keeps windowing independent of late column metadata', () => {
		const columns: string[] = [];
		const window = computeWindow(5, 0, 320, 32);
		assert.equal(columns.length, 0);
		assert.deepEqual(window, {
			start: 0,
			end: 5,
			topSpacer: 0,
			bottomSpacer: 0,
		});
	});
});

describe('estimateColumnWidths', () => {
	it('floors narrow content to the minimum width', () => {
		assert.deepEqual(
			estimateColumnWidths(['a', 'bb'], [{ a: 'x', bb: 'yz' }], identity),
			[STREAM_MIN_COLUMN_WIDTH, STREAM_MIN_COLUMN_WIDTH],
		);
	});

	it('grows with content and caps at the maximum width', () => {
		const widths = estimateColumnWidths(
			['short', 'long'],
			[{ short: 'x'.repeat(20), long: 'y'.repeat(100) }],
			identity,
		);
		assert.equal(widths[0], Math.ceil(20 * 7.5 + 24));
		assert.equal(widths[1], STREAM_MAX_COLUMN_WIDTH);
	});

	it('returns no widths when column metadata has not arrived yet', () => {
		assert.deepEqual(estimateColumnWidths([], [{ a: 1 }], identity), []);
	});
});

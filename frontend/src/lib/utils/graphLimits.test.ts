import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import {
	LARGE_GRAPH_ELEMENTS,
	MAX_GRAPH_EDGES,
	MAX_GRAPH_NODES,
	resolveCanvasLayout,
	shouldSimplify,
	truncateToLimit,
} from './graphLimits.ts';

describe('truncateToLimit', () => {
	it('keeps small collections untouched', () => {
		assert.deepEqual(truncateToLimit([1, 2], MAX_GRAPH_NODES), {
			items: [1, 2],
			total: 2,
			truncated: false,
		});
	});

	it('cuts large collections in order and records the total', () => {
		const items = Array.from({ length: MAX_GRAPH_NODES + 5 }, (_, i) => i);
		const result = truncateToLimit(items, MAX_GRAPH_NODES);
		assert.equal(result.items.length, MAX_GRAPH_NODES);
		assert.deepEqual(result.items.slice(0, 3), [0, 1, 2]);
		assert.equal(result.total, MAX_GRAPH_NODES + 5);
		assert.equal(result.truncated, true);
	});

	it('treats the exact limit as not truncated', () => {
		const items = Array.from({ length: MAX_GRAPH_EDGES }, (_, i) => i);
		const result = truncateToLimit(items, MAX_GRAPH_EDGES);
		assert.equal(result.truncated, false);
		assert.equal(result.total, MAX_GRAPH_EDGES);
	});
});

describe('shouldSimplify', () => {
	it('enables simplified rendering only above the large-graph line', () => {
		assert.equal(shouldSimplify(LARGE_GRAPH_ELEMENTS), false);
		assert.equal(shouldSimplify(LARGE_GRAPH_ELEMENTS + 1), true);
	});
});

describe('resolveCanvasLayout', () => {
	it('degrades large force graphs to grid', () => {
		assert.equal(
			resolveCanvasLayout('force', LARGE_GRAPH_ELEMENTS + 1),
			'grid',
		);
	});

	it('keeps small force graphs and explicit choices', () => {
		assert.equal(resolveCanvasLayout('force', 10), 'force');
		assert.equal(
			resolveCanvasLayout('grid', LARGE_GRAPH_ELEMENTS + 100),
			'grid',
		);
		assert.equal(
			resolveCanvasLayout('circle', LARGE_GRAPH_ELEMENTS + 100),
			'circle',
		);
	});
});

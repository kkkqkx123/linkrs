import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import {
	classifyPayload,
	recordColumns,
} from './structuredPayload.ts';

describe('classifyPayload', () => {
	it('classifies arrays of objects as records', () => {
		const result = classifyPayload([
			{ space_id: 1, tag_name: 'person' },
			{ space_id: 2, tag_name: 'doc' },
		]);
		assert.equal(result.shape, 'records');
		assert.equal(result.records.length, 2);
	});

	it('classifies arrays of scalars as strings', () => {
		const result = classifyPayload(['idx_a', 'idx_b']);
		assert.equal(result.shape, 'strings');
		assert.deepEqual(result.values, ['idx_a', 'idx_b']);
	});

	it('treats an empty array as an empty string list', () => {
		const result = classifyPayload([]);
		assert.equal(result.shape, 'strings');
		assert.deepEqual(result.values, []);
	});

	it('classifies primitives', () => {
		assert.deepEqual(classifyPayload(true), {
			shape: 'primitive',
			records: [],
			values: ['true'],
		});
	});

	it('unwraps an object envelope holding a list field', () => {
		const result = classifyPayload({
			indexes: [{ space_id: 1, status: 'ready' }],
		});
		assert.equal(result.shape, 'records');
		assert.equal(result.records[0].status, 'ready');
	});

	it('falls back to unknown for null, undefined and mixed arrays', () => {
		assert.equal(classifyPayload(null).shape, 'unknown');
		assert.equal(classifyPayload(undefined).shape, 'unknown');
		assert.equal(
			classifyPayload([{ id: 1 }, 'mixed']).shape,
			'unknown',
		);
		assert.equal(classifyPayload({ other: 1 }).shape, 'unknown');
	});
});

describe('recordColumns', () => {
	it('unions keys across rows preserving first-seen order', () => {
		const columns = recordColumns([
			{ b: 1, a: 1 },
			{ c: 2, a: 3 },
		]);
		assert.deepEqual(columns, ['b', 'a', 'c']);
	});

	it('returns an empty list for no records', () => {
		assert.deepEqual(recordColumns([]), []);
	});
});

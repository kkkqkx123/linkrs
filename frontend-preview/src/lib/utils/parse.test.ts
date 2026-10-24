import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { asArray, asRecord, isRecord, pickList } from './parse.ts';

describe('isRecord', () => {
	it('accepts plain objects and rejects the rest', () => {
		assert.equal(isRecord({ a: 1 }), true);
		assert.equal(isRecord(null), false);
		assert.equal(isRecord([1, 2]), false);
		assert.equal(isRecord('x'), false);
	});
});

describe('asRecord', () => {
	it('returns the record or an empty object', () => {
		assert.deepEqual(asRecord({ a: 1 }), { a: 1 });
		assert.deepEqual(asRecord(null), {});
		assert.deepEqual(asRecord(42), {});
	});
});

describe('asArray', () => {
	it('keeps only record items', () => {
		assert.deepEqual(asArray([{ a: 1 }, null, 2, { b: 3 }]), [
			{ a: 1 },
			{ b: 3 },
		]);
		assert.deepEqual(asArray({}), []);
	});
});

describe('pickList', () => {
	it('pulls lists from bare, enveloped and raw shapes', () => {
		const rows = [{ id: 1 }];
		assert.deepEqual(pickList({ spaces: rows }, ['spaces']), rows);
		assert.deepEqual(pickList({ data: { tags: rows } }, ['tags']), rows);
		assert.deepEqual(pickList(rows, ['spaces']), rows);
		assert.deepEqual(pickList({ other: 1 }, ['spaces']), []);
	});
});

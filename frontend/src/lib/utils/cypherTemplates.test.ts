import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import {
	buildEdgeDelete,
	buildEdgeUpdate,
	buildVertexDelete,
	buildVertexUpdate,
	formatCypherValue,
	quoteVid,
} from './cypherTemplates.ts';

describe('formatCypherValue', () => {
	it('renders primitives and null', () => {
		assert.equal(formatCypherValue(null), 'NULL');
		assert.equal(formatCypherValue(3), '3');
		assert.equal(formatCypherValue(true), 'true');
		assert.equal(formatCypherValue('Alice'), '"Alice"');
	});

	it('keeps numeric strings unquoted for vid-style values', () => {
		assert.equal(formatCypherValue('101'), '101');
		assert.equal(quoteVid('101'), '101');
		assert.equal(quoteVid('abc'), '"abc"');
	});
});

describe('vertex templates', () => {
	it('builds update and delete statements', () => {
		assert.equal(
			buildVertexUpdate('101', { name: 'Alice', age: 26 }),
			'UPDATE VERTEX 101 SET name = "Alice", age = 26',
		);
		assert.equal(buildVertexDelete('101'), 'DELETE VERTEX 101 WITH EDGE');
	});
});

describe('edge templates', () => {
	it('builds update and delete statements', () => {
		assert.equal(
			buildEdgeUpdate('follow', '101', '102', 0, { degree: 0.9 }),
			'UPDATE EDGE 101 -> 102 @0 OF follow SET degree = 0.9',
		);
		assert.equal(
			buildEdgeDelete('follow', '101', '102', 0),
			'DELETE EDGE 101 -> 102 @0 OF follow',
		);
	});
});

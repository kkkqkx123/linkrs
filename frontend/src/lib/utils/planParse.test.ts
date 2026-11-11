import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { buildPlanTree, parsePlanTable, type PlanNode } from './planParse.ts';

// Column metrics mirror the backend table renderer: id 4 (right-aligned),
// name 16, deps 10, profiling_data 48, operator_info 48, output_var 16.
const SEPARATOR =
	'+------+------------------+------------+--------------------------------------------------+--------------------------------------------------+------------------+';
const HEADER =
	'| id   | name             | deps       | profiling_data                                    | operator_info                                    | output_var       |';

function row(
	id: string,
	name: string,
	deps: string,
	profiling: string,
	info: string,
	outputVar: string,
): string {
	return `| ${id.padStart(4)} | ${name.padEnd(16)} | ${deps.padEnd(10)} | ${profiling.padEnd(48)} | ${info.padEnd(48)} | ${outputVar.padEnd(16)} |`;
}

const SAMPLE_TABLE = [
	'Execution Model: Batch Pull',
	'Parallel Workers: requested=1',
	'',
	SEPARATOR,
	HEADER,
	SEPARATOR,
	row('0', 'ResultSink', '1', '-', 'est_rows:2', '__Value_2'),
	SEPARATOR,
	row(
		'1',
		'Filter',
		'2',
		'-',
		'predicate:(n.age > 30),est_rows:12',
		'__Filter_1',
	),
	SEPARATOR,
	row(
		'2',
		'ScanVertices',
		'-',
		'rows:100,time:12us',
		'table:Person,est_rows:100',
		'__Value_0',
	),
	SEPARATOR,
].join('\n');

function node(partial: Partial<PlanNode> & { id: number }): PlanNode {
	return {
		name: '',
		deps: [],
		profilingData: '',
		operatorInfo: '',
		outputVar: '',
		...partial,
	};
}

describe('parsePlanTable', () => {
	it('parses a full ASCII plan table into ordered nodes', () => {
		const nodes = parsePlanTable(SAMPLE_TABLE);
		assert.ok(nodes);
		assert.equal(nodes.length, 3);
		assert.deepEqual(nodes[0], {
			id: 0,
			name: 'ResultSink',
			deps: [1],
			profilingData: '-',
			operatorInfo: 'est_rows:2',
			outputVar: '__Value_2',
		});
		assert.deepEqual(nodes[2], {
			id: 2,
			name: 'ScanVertices',
			deps: [],
			profilingData: 'rows:100,time:12us',
			operatorInfo: 'table:Person,est_rows:100',
			outputVar: '__Value_0',
		});
	});

	it('fills missing trailing columns with empty strings', () => {
		const table = [
			SEPARATOR,
			HEADER,
			SEPARATOR,
			'|    3 | Filter          | 2 |',
			SEPARATOR,
		].join('\n');
		const nodes = parsePlanTable(table);
		assert.ok(nodes);
		assert.equal(nodes.length, 1);
		assert.deepEqual(nodes[0], {
			id: 3,
			name: 'Filter',
			deps: [2],
			profilingData: '',
			operatorInfo: '',
			outputVar: '',
		});
	});

	it('tolerates spaced dependency lists and non-numeric dep tokens', () => {
		const table = [
			SEPARATOR,
			HEADER,
			SEPARATOR,
			row(
				'4',
				'Aggregate',
				'2, 3',
				'rows:5,time:7us',
				'est_rows:5.5',
				'__Value_3',
			),
			SEPARATOR,
			row('5', 'Dedup', '1,x', '-', 'est_rows:4', '__Value_4'),
			SEPARATOR,
		].join('\n');
		const nodes = parsePlanTable(table);
		assert.ok(nodes);
		assert.deepEqual(nodes[0]?.deps, [2, 3]);
		assert.deepEqual(nodes[1]?.deps, [1]);
	});

	it('returns null for text that is not a plan table', () => {
		assert.equal(parsePlanTable(''), null);
		assert.equal(parsePlanTable('MATCH (n:Person) RETURN n'), null);
		assert.equal(parsePlanTable('digraph G {\n  1[label="Scan"];\n}'), null);
		// A regular result table has pipes but no plan header columns.
		assert.equal(parsePlanTable('| vid | name |\n| --- | ---- |'), null);
	});

	it('returns an empty node list for a valid table without rows', () => {
		const table = [SEPARATOR, HEADER, SEPARATOR].join('\n');
		assert.deepEqual(parsePlanTable(table), []);
	});
});

describe('buildPlanTree', () => {
	it('mounts consumers below their first dependency only', () => {
		const tree = buildPlanTree([
			node({ id: 0, name: 'Aggregate', deps: [1, 2] }),
			node({ id: 1, name: 'ScanA', deps: [] }),
			node({ id: 2, name: 'ScanB', deps: [] }),
		]);
		assert.deepEqual(
			tree.map((root) => root.id),
			[1, 2],
		);
		assert.deepEqual(
			tree[0]?.children.map((child) => child.id),
			[0],
		);
		assert.deepEqual(tree[1]?.children, []);
	});

	it('keeps nodes with multiple deps from being duplicated', () => {
		const tree = buildPlanTree([
			node({ id: 7, name: 'Join', deps: [2, 1] }),
			node({ id: 1, name: 'ScanA', deps: [] }),
			node({ id: 2, name: 'ScanB', deps: [] }),
		]);
		assert.deepEqual(
			tree.map((root) => root.id),
			[1, 2],
		);
		// The join hangs below its first dep (2) and never below 1.
		assert.deepEqual(
			tree[1]?.children.map((child) => child.id),
			[7],
		);
		assert.deepEqual(tree[0]?.children, []);
	});

	it('promotes orphan nodes whose deps reference unknown ids to roots', () => {
		const tree = buildPlanTree([
			node({ id: 0, name: 'Aggregate', deps: [1, 2] }),
			node({ id: 1, name: 'ScanA', deps: [] }),
			node({ id: 2, name: 'ScanB', deps: [] }),
			node({ id: 5, name: 'Dangling', deps: [99] }),
		]);
		assert.deepEqual(
			tree.map((root) => root.id),
			[1, 2, 5],
		);
	});

	it('chains a parsed single-lineage plan from leaf to sink', () => {
		const nodes = parsePlanTable(SAMPLE_TABLE);
		assert.ok(nodes);
		const tree = buildPlanTree(nodes);
		assert.deepEqual(
			tree.map((root) => root.id),
			[2],
		);
		assert.deepEqual(
			tree[0]?.children.map((child) => child.id),
			[1],
		);
		assert.deepEqual(
			tree[0]?.children[0]?.children.map((child) => child.id),
			[0],
		);
	});
});

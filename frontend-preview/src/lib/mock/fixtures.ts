/**
 * Shared mock fixtures: a demo social graph used by the graph, query and
 * data-browser handlers so every page shows a coherent dataset.
 * Vertex ids are deliberately large integers (as strings) to exercise the
 * json-bigint path in the API client.
 */

export interface MockVertex {
	id: string;
	tag: string;
	properties: Record<string, unknown>;
}

export interface MockEdge {
	id: string;
	type: string;
	src: string;
	dst: string;
	rank: number;
	properties: Record<string, unknown>;
}

export const demoSpace = 'social';

export const demoTags = [
	{
		id: 1,
		name: 'person',
		properties: [
			{
				name: 'name',
				data_type: 'STRING',
				nullable: false,
				default_value: null,
			},
			{ name: 'age', data_type: 'INT64', nullable: true, default_value: null },
		],
		comment: 'demo person tag',
		created_at: 1735689600,
	},
	{
		id: 2,
		name: 'company',
		properties: [
			{
				name: 'title',
				data_type: 'STRING',
				nullable: false,
				default_value: null,
			},
		],
		comment: '',
		created_at: 1735689660,
	},
];

export const demoEdgeTypes = [
	{
		id: 1,
		name: 'knows',
		properties: [
			{
				name: 'since',
				data_type: 'DATETIME',
				nullable: true,
				default_value: null,
			},
		],
		comment: '',
		created_at: 1735689700,
	},
	{
		id: 2,
		name: 'works_at',
		properties: [],
		comment: '',
		created_at: 1735689710,
	},
];

export const demoVertices: MockVertex[] = [
	{
		id: '9007199254740993',
		tag: 'person',
		properties: { name: 'Alice', age: 30 },
	},
	{
		id: '9007199254740994',
		tag: 'person',
		properties: { name: 'Bob', age: 41 },
	},
	{
		id: '9007199254740995',
		tag: 'person',
		properties: { name: 'Carol', age: 25 },
	},
	{
		id: '9007199254740996',
		tag: 'company',
		properties: { title: 'Linkrs Inc.' },
	},
];

export const demoEdges: MockEdge[] = [
	{
		id: 'e1',
		type: 'knows',
		src: '9007199254740993',
		dst: '9007199254740994',
		rank: 0,
		properties: { since: '2021-03-01T00:00:00' },
	},
	{
		id: 'e2',
		type: 'knows',
		src: '9007199254740993',
		dst: '9007199254740995',
		rank: 0,
		properties: { since: '2022-07-15T00:00:00' },
	},
	{
		id: 'e3',
		type: 'works_at',
		src: '9007199254740994',
		dst: '9007199254740996',
		rank: 0,
		properties: {},
	},
];

export function findVertex(vid: string): MockVertex | undefined {
	return demoVertices.find((v) => v.id === vid);
}

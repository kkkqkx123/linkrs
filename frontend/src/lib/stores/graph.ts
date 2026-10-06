import { writable } from 'svelte/store';
import type {
	LayoutType,
	NodeDetail,
	EdgeDetail,
} from '$types/graph';
import type { GraphData } from '$types/graph';

export type { NodeDetail, EdgeDetail };

export interface NodeStyle {
	color: string;
	size: 'small' | 'medium' | 'large';
	labelProperty: string;
}

export interface EdgeStyle {
	color: string;
	width: 'thin' | 'medium' | 'thick';
	labelProperty: string;
}

export interface GraphState {
	graphData: GraphData | null;
	layout: LayoutType;
	zoom: number;
	selectedNodes: string[];
	selectedEdges: string[];
	nodeStyles: Record<string, NodeStyle>;
	edgeStyles: Record<string, EdgeStyle>;
	detailPanelVisible: boolean;
	detailData: NodeDetail | EdgeDetail | null;
	detailType: 'node' | 'edge' | null;
	searchQuery: string;
	filterTags: Set<string>;
	filterEdgeTypes: Set<string>;
	simplifiedMode: boolean;
	layoutParams: {
		nodeRepulsion: number;
		gravity: number;
		numIter: number;
	};
}

const defaultNodeStyle: NodeStyle = {
	color: '#1890ff',
	size: 'medium',
	labelProperty: 'id',
};
const defaultEdgeStyle: EdgeStyle = {
	color: '#999',
	width: 'medium',
	labelProperty: 'type',
};

const generateNodeColor = (index: number): string => {
	const colors = [
		'#1890ff',
		'#52c41a',
		'#faad14',
		'#f5222d',
		'#722ed1',
		'#13c2c2',
		'#eb2f96',
		'#fa8c16',
	];
	return colors[index % colors.length];
};

const generateEdgeColor = (index: number): string => {
	const colors = ['#999', '#666', '#333', '#1890ff', '#52c41a'];
	return colors[index % colors.length];
};

function loadPersisted(): Partial<GraphState> {
	try {
		const saved = localStorage.getItem('graph-storage');
		if (saved) return JSON.parse(saved);
	} catch {
		/* ignore */
	}
	return {};
}

const persisted = loadPersisted();

function persistStyles(state: GraphState) {
	localStorage.setItem(
		'graph-storage',
		JSON.stringify({
			layout: state.layout,
			nodeStyles: state.nodeStyles,
			edgeStyles: state.edgeStyles,
		}),
	);
}

function ensureStylesForData(
	nodeStyles: Record<string, NodeStyle>,
	edgeStyles: Record<string, EdgeStyle>,
	data: GraphData,
): { nodeStyles: Record<string, NodeStyle>; edgeStyles: Record<string, EdgeStyle> } {
	const newNodeStyles = { ...nodeStyles };
	const newEdgeStyles = { ...edgeStyles };
	let nodeColorIndex = Object.keys(nodeStyles).length;
	data.nodes.forEach((node) => {
		if (!newNodeStyles[node.tag])
			newNodeStyles[node.tag] = {
				...defaultNodeStyle,
				color: generateNodeColor(nodeColorIndex++),
			};
	});
	let edgeColorIndex = Object.keys(edgeStyles).length;
	data.edges.forEach((edge) => {
		if (!newEdgeStyles[edge.type])
			newEdgeStyles[edge.type] = {
				...defaultEdgeStyle,
				color: generateEdgeColor(edgeColorIndex++),
			};
	});
	return { nodeStyles: newNodeStyles, edgeStyles: newEdgeStyles };
}

function createGraphStore() {
	const { subscribe, update } = writable<GraphState>({
		graphData: null,
		layout: (persisted.layout as LayoutType) || 'force',
		zoom: 1,
		selectedNodes: [],
		selectedEdges: [],
		nodeStyles: (persisted.nodeStyles as Record<string, NodeStyle>) || {},
		edgeStyles: (persisted.edgeStyles as Record<string, EdgeStyle>) || {},
		detailPanelVisible: false,
		detailData: null,
		detailType: null,
		searchQuery: '',
		filterTags: new Set<string>(),
		filterEdgeTypes: new Set<string>(),
		simplifiedMode: false,
		layoutParams: {
			nodeRepulsion: 4500,
			gravity: 0.1,
			numIter: 1500,
		},
	});

	// Use Map for O(1) deduplication during merge operations
	let nodeMap = new Map<string, GraphData['nodes'][number]>();
	let edgeMap = new Map<string, GraphData['edges'][number]>();

	function rebuildMaps(data: GraphData | null) {
		nodeMap = new Map();
		edgeMap = new Map();
		if (data) {
			for (const node of data.nodes) nodeMap.set(node.id, node);
			for (const edge of data.edges) edgeMap.set(edge.id, edge);
		}
	}

	return {
		subscribe,
		setGraphData: (data: GraphData) =>
			update((s) => {
				const { nodeStyles, edgeStyles } = ensureStylesForData(
					s.nodeStyles,
					s.edgeStyles,
					data,
				);
				rebuildMaps(data);
				const newState = {
					...s,
					graphData: data,
					nodeStyles,
					edgeStyles,
				};
				persistStyles(newState);
				return newState;
			}),
		clearGraphData: () =>
			update((s) => ({
				...s,
				graphData: null,
				selectedNodes: [],
				selectedEdges: [],
			})),
		mergeGraphData: (data: GraphData) =>
			update((s) => {
				const nodes = [...(s.graphData?.nodes ?? [])];
				const edges = [...(s.graphData?.edges ?? [])];
				for (const node of data.nodes) {
					if (!nodeMap.has(node.id)) {
						nodeMap.set(node.id, node);
						nodes.push(node);
					}
				}
				for (const edge of data.edges) {
					if (!edgeMap.has(edge.id)) {
						edgeMap.set(edge.id, edge);
						edges.push(edge);
					}
				}
				const merged: GraphData = { nodes, edges };
				const { nodeStyles, edgeStyles } = ensureStylesForData(
					s.nodeStyles,
					s.edgeStyles,
					merged,
				);
				const newState = {
					...s,
					graphData: merged,
					nodeStyles,
					edgeStyles,
				};
				persistStyles(newState);
				return newState;
			}),
		hasNode: (id: string): boolean => nodeMap.has(id),
		hasEdge: (id: string): boolean => edgeMap.has(id),
		setLayout: (layout: LayoutType) => update((s) => ({ ...s, layout })),
		setZoom: (zoom: number) => update((s) => ({ ...s, zoom })),
		selectNode: (id: string, multi = false) =>
			update((s) => {
				if (multi) {
					const index = s.selectedNodes.indexOf(id);
					return {
						...s,
						selectedNodes:
							index > -1
								? s.selectedNodes.filter((n) => n !== id)
								: [...s.selectedNodes, id],
					};
				}
				return { ...s, selectedNodes: [id], selectedEdges: [] };
			}),
		selectEdge: (id: string, multi = false) =>
			update((s) => {
				if (multi) {
					const index = s.selectedEdges.indexOf(id);
					return {
						...s,
						selectedEdges:
							index > -1
								? s.selectedEdges.filter((e) => e !== id)
								: [...s.selectedEdges, id],
					};
				}
				return { ...s, selectedEdges: [id], selectedNodes: [] };
			}),
		clearSelection: () =>
			update((s) => ({ ...s, selectedNodes: [], selectedEdges: [] })),
		setNodeStyle: (tag: string, style: Partial<NodeStyle>) =>
			update((s) => {
				const next = {
					...s,
					nodeStyles: {
						...s.nodeStyles,
						[tag]: { ...(s.nodeStyles[tag] || defaultNodeStyle), ...style },
					},
				};
				persistStyles(next);
				return next;
			}),
		setEdgeStyle: (type: string, style: Partial<EdgeStyle>) =>
			update((s) => {
				const next = {
					...s,
					edgeStyles: {
						...s.edgeStyles,
						[type]: { ...(s.edgeStyles[type] || defaultEdgeStyle), ...style },
					},
				};
				persistStyles(next);
				return next;
			}),
		resetStyles: () =>
			update((s) => {
				const next = { ...s, nodeStyles: {}, edgeStyles: {} };
				persistStyles(next);
				return next;
			}),
		showDetail: (data: NodeDetail | EdgeDetail, type: 'node' | 'edge') =>
			update((s) => ({
				...s,
				detailData: data,
				detailType: type,
				detailPanelVisible: true,
			})),
		hideDetail: () =>
			update((s) => ({
				...s,
				detailPanelVisible: false,
				detailData: null,
				detailType: null,
			})),
		setSearchQuery: (query: string) =>
			update((s) => ({ ...s, searchQuery: query })),
		toggleFilterTag: (tag: string) =>
			update((s) => {
				const next = new Set(s.filterTags);
				if (next.has(tag)) next.delete(tag);
				else next.add(tag);
				return { ...s, filterTags: next };
			}),
		toggleFilterEdgeType: (type: string) =>
			update((s) => {
				const next = new Set(s.filterEdgeTypes);
				if (next.has(type)) next.delete(type);
				else next.add(type);
				return { ...s, filterEdgeTypes: next };
			}),
		clearFilters: () =>
			update((s) => ({
				...s,
				searchQuery: '',
				filterTags: new Set<string>(),
				filterEdgeTypes: new Set<string>(),
			})),
		setSimplifiedMode: (enabled: boolean) =>
			update((s) => ({ ...s, simplifiedMode: enabled })),
		setLayoutParams: (params: Partial<{ nodeRepulsion: number; gravity: number; numIter: number }>) =>
			update((s) => ({
				...s,
				layoutParams: { ...s.layoutParams, ...params },
			})),
	};
}

export const graphStore = createGraphStore();

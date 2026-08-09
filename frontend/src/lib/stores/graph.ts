import { writable } from 'svelte/store';
import type { GraphData, LayoutType } from '$types/graph';

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

export interface NodeDetail {
  id: string;
  tag: string;
  properties: Record<string, unknown>;
}

export interface EdgeDetail {
  id: string;
  type: string;
  source: string;
  target: string;
  rank: number;
  properties: Record<string, unknown>;
}

interface GraphState {
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
}

const defaultNodeStyle: NodeStyle = { color: '#1890ff', size: 'medium', labelProperty: 'id' };
const defaultEdgeStyle: EdgeStyle = { color: '#999', width: 'medium', labelProperty: 'type' };

const generateNodeColor = (index: number): string => {
  const colors = ['#1890ff', '#52c41a', '#faad14', '#f5222d', '#722ed1', '#13c2c2', '#eb2f96', '#fa8c16'];
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
  } catch { /* ignore */ }
  return {};
}

const persisted = loadPersisted();

function createGraphStore() {
  const { subscribe, set, update } = writable<GraphState>({
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
  });

  return {
    subscribe,
    setGraphData: (data: GraphData) => update(s => {
      const newNodeStyles = { ...s.nodeStyles };
      const newEdgeStyles = { ...s.edgeStyles };
      let nodeColorIndex = Object.keys(s.nodeStyles).length;
      data.nodes.forEach(node => {
        if (!newNodeStyles[node.tag]) newNodeStyles[node.tag] = { ...defaultNodeStyle, color: generateNodeColor(nodeColorIndex++) };
      });
      let edgeColorIndex = Object.keys(s.edgeStyles).length;
      data.edges.forEach(edge => {
        if (!newEdgeStyles[edge.type]) newEdgeStyles[edge.type] = { ...defaultEdgeStyle, color: generateEdgeColor(edgeColorIndex++) };
      });
      const newState = { ...s, graphData: data, nodeStyles: newNodeStyles, edgeStyles: newEdgeStyles };
      localStorage.setItem('graph-storage', JSON.stringify({ layout: newState.layout, nodeStyles: newState.nodeStyles, edgeStyles: newState.edgeStyles }));
      return newState;
    }),
    clearGraphData: () => update(s => ({ ...s, graphData: null, selectedNodes: [], selectedEdges: [] })),
    setLayout: (layout: LayoutType) => update(s => ({ ...s, layout })),
    setZoom: (zoom: number) => update(s => ({ ...s, zoom })),
    selectNode: (id: string, multi = false) => update(s => {
      if (multi) {
        const index = s.selectedNodes.indexOf(id);
        return { ...s, selectedNodes: index > -1 ? s.selectedNodes.filter(n => n !== id) : [...s.selectedNodes, id] };
      }
      return { ...s, selectedNodes: [id], selectedEdges: [] };
    }),
    selectEdge: (id: string, multi = false) => update(s => {
      if (multi) {
        const index = s.selectedEdges.indexOf(id);
        return { ...s, selectedEdges: index > -1 ? s.selectedEdges.filter(e => e !== id) : [...s.selectedEdges, id] };
      }
      return { ...s, selectedEdges: [id], selectedNodes: [] };
    }),
    clearSelection: () => update(s => ({ ...s, selectedNodes: [], selectedEdges: [] })),
    setNodeStyle: (tag: string, style: Partial<NodeStyle>) => update(s => ({
      ...s, nodeStyles: { ...s.nodeStyles, [tag]: { ...(s.nodeStyles[tag] || defaultNodeStyle), ...style } },
    })),
    setEdgeStyle: (type: string, style: Partial<EdgeStyle>) => update(s => ({
      ...s, edgeStyles: { ...s.edgeStyles, [type]: { ...(s.edgeStyles[type] || defaultEdgeStyle), ...style } },
    })),
    resetStyles: () => update(s => ({ ...s, nodeStyles: {}, edgeStyles: {} })),
    showDetail: (data: NodeDetail | EdgeDetail, type: 'node' | 'edge') => update(s => ({ ...s, detailData: data, detailType: type, detailPanelVisible: true })),
    hideDetail: () => update(s => ({ ...s, detailPanelVisible: false, detailData: null, detailType: null })),
  };
}

export const graphStore = createGraphStore();
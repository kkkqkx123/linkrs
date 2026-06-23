import { create } from 'zustand';
import { persist } from 'zustand/middleware';
import type { GraphData, LayoutType } from '@/types/graph';

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

export interface GraphState {
  // Graph data
  graphData: GraphData | null;

  // View state
  layout: LayoutType;
  zoom: number;
  selectedNodes: string[];
  selectedEdges: string[];

  // Style configuration
  nodeStyles: Record<string, NodeStyle>;
  edgeStyles: Record<string, EdgeStyle>;

  // Detail panel
  detailPanelVisible: boolean;
  detailData: NodeDetail | EdgeDetail | null;
  detailType: 'node' | 'edge' | null;

  // Actions
  setGraphData: (data: GraphData) => void;
  clearGraphData: () => void;
  setLayout: (layout: LayoutType) => void;
  setZoom: (zoom: number) => void;
  selectNode: (id: string, multi?: boolean) => void;
  selectEdge: (id: string, multi?: boolean) => void;
  clearSelection: () => void;
  setNodeStyle: (tag: string, style: Partial<NodeStyle>) => void;
  setEdgeStyle: (type: string, style: Partial<EdgeStyle>) => void;
  resetStyles: () => void;
  showDetail: (data: NodeDetail | EdgeDetail, type: 'node' | 'edge') => void;
  hideDetail: () => void;
  fitToScreen: (cy?: cytoscape.Core) => void;
  resetLayout: (cy?: cytoscape.Core) => void;
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
  const colors = ['#1890ff', '#52c41a', '#faad14', '#f5222d', '#722ed1', '#13c2c2', '#eb2f96', '#fa8c16'];
  return colors[index % colors.length];
};

const generateEdgeColor = (index: number): string => {
  const colors = ['#999', '#666', '#333', '#1890ff', '#52c41a'];
  return colors[index % colors.length];
};

export const useGraphStore = create<GraphState>()(
  persist(
    (set, get) => ({
      // Initial state
      graphData: null,
      layout: 'force',
      zoom: 1,
      selectedNodes: [],
      selectedEdges: [],
      nodeStyles: {},
      edgeStyles: {},
      detailPanelVisible: false,
      detailData: null,
      detailType: null,

      // Actions
      setGraphData: (data) => {
        set({ graphData: data });

        // Auto-generate styles for new tags and edge types
        const { nodeStyles, edgeStyles } = get();
        const newNodeStyles = { ...nodeStyles };
        const newEdgeStyles = { ...edgeStyles };

        let nodeColorIndex = Object.keys(nodeStyles).length;
        data.nodes.forEach((node) => {
          if (!newNodeStyles[node.tag]) {
            newNodeStyles[node.tag] = {
              ...defaultNodeStyle,
              color: generateNodeColor(nodeColorIndex++),
            };
          }
        });

        let edgeColorIndex = Object.keys(edgeStyles).length;
        data.edges.forEach((edge) => {
          if (!newEdgeStyles[edge.type]) {
            newEdgeStyles[edge.type] = {
              ...defaultEdgeStyle,
              color: generateEdgeColor(edgeColorIndex++),
            };
          }
        });

        set({ nodeStyles: newNodeStyles, edgeStyles: newEdgeStyles });
      },

      clearGraphData: () => set({ graphData: null, selectedNodes: [], selectedEdges: [] }),

      setLayout: (layout) => set({ layout }),

      setZoom: (zoom) => set({ zoom }),

      selectNode: (id, multi = false) => {
        const { selectedNodes } = get();
        if (multi) {
          const index = selectedNodes.indexOf(id);
          if (index > -1) {
            set({ selectedNodes: selectedNodes.filter((n) => n !== id) });
          } else {
            set({ selectedNodes: [...selectedNodes, id] });
          }
        } else {
          set({ selectedNodes: [id], selectedEdges: [] });
        }
      },

      selectEdge: (id, multi = false) => {
        const { selectedEdges } = get();
        if (multi) {
          const index = selectedEdges.indexOf(id);
          if (index > -1) {
            set({ selectedEdges: selectedEdges.filter((e) => e !== id) });
          } else {
            set({ selectedEdges: [...selectedEdges, id] });
          }
        } else {
          set({ selectedEdges: [id], selectedNodes: [] });
        }
      },

      clearSelection: () => set({ selectedNodes: [], selectedEdges: [] }),

      setNodeStyle: (tag, style) => {
        const { nodeStyles } = get();
        set({
          nodeStyles: {
            ...nodeStyles,
            [tag]: { ...(nodeStyles[tag] || defaultNodeStyle), ...style },
          },
        });
      },

      setEdgeStyle: (type, style) => {
        const { edgeStyles } = get();
        set({
          edgeStyles: {
            ...edgeStyles,
            [type]: { ...(edgeStyles[type] || defaultEdgeStyle), ...style },
          },
        });
      },

      resetStyles: () => set({ nodeStyles: {}, edgeStyles: {} }),

      showDetail: (data, type) => set({ detailData: data, detailType: type, detailPanelVisible: true }),

      hideDetail: () => set({ detailPanelVisible: false, detailData: null, detailType: null }),

      fitToScreen: (cy) => {
        if (cy) {
          cy.fit();
          set({ zoom: cy.zoom() });
        }
      },

      resetLayout: (cy) => {
        const { layout } = get();
        if (cy) {
          cy.layout({ name: layout, padding: 10 }).run();
        }
      },
    }),
    {
      name: 'graph-storage',
      partialize: (state) => ({
        layout: state.layout,
        nodeStyles: state.nodeStyles,
        edgeStyles: state.edgeStyles,
      }),
    }
  )
);

import React, { useEffect, useRef, useCallback } from 'react';
import cytoscape from 'cytoscape';
import { useGraphStore } from '@/stores/graph';
import { convertToCytoscapeElements, generateCytoscapeStyle } from '@/utils/cytoscapeConfig';
import { applyLayout } from '@/utils/graphLayout';
import styles from './index.module.less';

interface GraphCanvasProps {
  data?: import('@/types/graph').GraphData;
  height?: string;
}

const GraphCanvas: React.FC<GraphCanvasProps> = ({ data, height = '500px' }) => {
  const containerRef = useRef<HTMLDivElement>(null);
  const cyRef = useRef<cytoscape.Core | null>(null);

  const {
    graphData,
    layout,
    nodeStyles,
    edgeStyles,
    setGraphData,
    selectNode,
    selectEdge,
    showDetail,
    setZoom,
  } = useGraphStore();

  const currentData = data || graphData;

  // Initialize Cytoscape
  const initCytoscape = useCallback(() => {
    if (!containerRef.current || !currentData) return;

    // Destroy existing instance
    if (cyRef.current) {
      cyRef.current.destroy();
    }

    const styleConfig = { nodes: nodeStyles, edges: edgeStyles };

    const cy = cytoscape({
      container: containerRef.current,
      elements: convertToCytoscapeElements(currentData),
      style: generateCytoscapeStyle(styleConfig),
      layout: { name: 'preset' },
      minZoom: 0.1,
      maxZoom: 3,
      wheelSensitivity: 0.3,
    });

    cyRef.current = cy;

    // Apply initial layout
    applyLayout(cy, layout);

    // Event listeners
    cy.on('tap', 'node', (evt) => {
      const node = evt.target;
      const isMultiSelect = evt.originalEvent?.ctrlKey || evt.originalEvent?.metaKey;
      selectNode(node.id(), isMultiSelect);
      showDetail(
        {
          id: node.id(),
          tag: node.data('_tag'),
          properties: Object.fromEntries(
            Object.entries(node.data()).filter(([key]) => !key.startsWith('_'))
          ),
        },
        'node'
      );
    });

    cy.on('tap', 'edge', (evt) => {
      const edge = evt.target;
      const isMultiSelect = evt.originalEvent?.ctrlKey || evt.originalEvent?.metaKey;
      selectEdge(edge.id(), isMultiSelect);
      showDetail(
        {
          id: edge.id(),
          type: edge.data('_type'),
          source: edge.data('source'),
          target: edge.data('target'),
          rank: edge.data('_rank'),
          properties: Object.fromEntries(
            Object.entries(edge.data()).filter(([key]) => !key.startsWith('_') && key !== 'source' && key !== 'target' && key !== 'id' && key !== 'label')
          ),
        },
        'edge'
      );
    });

    cy.on('tap', (evt) => {
      if (evt.target === cy) {
        useGraphStore.getState().clearSelection();
      }
    });

    cy.on('zoom', () => {
      setZoom(cy.zoom());
    });

    // Warn if too many nodes
    if (currentData.nodes.length > 500) {
      console.warn(`Graph contains ${currentData.nodes.length} nodes. Performance may be affected.`);
    }
  }, [currentData, layout, nodeStyles, edgeStyles, selectNode, selectEdge, showDetail, setZoom]);

  // Initialize on mount and data change
  useEffect(() => {
    if (data) {
      setGraphData(data);
    }
  }, [data, setGraphData]);

  useEffect(() => {
    initCytoscape();

    return () => {
      if (cyRef.current) {
        cyRef.current.destroy();
        cyRef.current = null;
      }
    };
  }, [initCytoscape]);

  // Update styles when changed
  useEffect(() => {
    if (!cyRef.current) return;
    const styleConfig = { nodes: nodeStyles, edges: edgeStyles };
    cyRef.current.style().clear();
    cyRef.current.style(generateCytoscapeStyle(styleConfig));
  }, [nodeStyles, edgeStyles]);

  // Update layout when changed
  useEffect(() => {
    if (!cyRef.current) return;
    applyLayout(cyRef.current, layout);
  }, [layout]);

  // Expose cy instance for parent components
  useEffect(() => {
    if (cyRef.current) {
      (window as unknown as { cy: cytoscape.Core }).cy = cyRef.current;
    }
  }, []);

  if (!currentData || currentData.nodes.length === 0) {
    return (
      <div className={styles.empty} style={{ height }}>
        <p>No graph data to display</p>
        <p className={styles.hint}>Execute a query that returns nodes and edges to visualize the graph</p>
      </div>
    );
  }

  return (
    <div
      ref={containerRef}
      className={styles.canvas}
      style={{ height, width: '100%' }}
    />
  );
};

export default GraphCanvas;

import React, { useEffect, useRef } from 'react';
import cytoscape from 'cytoscape';
import dagre from 'cytoscape-dagre';
import type { GraphData, NodeData } from '../utils/graphBuilder';
import styles from './index.module.less';

// Register layout plugin
cytoscape.use(dagre);

interface GraphCanvasProps {
  data: GraphData;
  onNodeClick: (node: NodeData | null) => void;
}

const GraphCanvas: React.FC<GraphCanvasProps> = ({ data, onNodeClick }) => {
  const containerRef = useRef<HTMLDivElement>(null);
  const cyRef = useRef<cytoscape.Core | null>(null);

  useEffect(() => {
    if (!containerRef.current) return;

    // Initialize Cytoscape
    const cy = cytoscape({
      container: containerRef.current,
      elements: data.elements,
      style: getSchemaGraphStyle(),
      layout: {
        name: 'dagre',
        rankDir: 'TB',
        nodeSep: 80,
        edgeSep: 40,
        rankSep: 100,
        padding: 20,
      } as cytoscape.LayoutOptions,
      minZoom: 0.2,
      maxZoom: 3,
      wheelSensitivity: 0.3,
    });

    cyRef.current = cy;

    // Node click event
    cy.on('tap', 'node', (evt) => {
      const node = evt.target;
      onNodeClick({
        id: node.id(),
        type: node.data('type'),
        name: node.data('name'),
        properties: node.data('properties'),
        comment: node.data('comment'),
        ttl: node.data('ttl'),
      });
    });

    // Click on empty area to deselect
    cy.on('tap', (evt) => {
      if (evt.target === cy) {
        onNodeClick(null);
      }
    });

    return () => {
      cy.destroy();
      cyRef.current = null;
    };
  }, [data, onNodeClick]);

  return (
    <div
      ref={containerRef}
      className={styles.graphCanvas}
      style={{ width: '100%', height: '100%' }}
    />
  );
};

// Schema graph style configuration
const getSchemaGraphStyle = (): cytoscape.StylesheetCSS[] => [
  {
    selector: 'node',
    css: {
      'background-color': '#fff',
      'border-width': 2,
      'border-color': '#1890ff',
      'width': 120,
      'height': 80,
      'shape': 'roundrectangle',
      'label': 'data(name)',
      'text-valign': 'center',
      'text-halign': 'center',
      'font-size': '14px',
      'font-weight': 'bold',
      'color': '#1890ff',
    },
  },
  {
    selector: 'node[type="tag"]',
    css: {
      'border-color': '#52c41a',
      'color': '#52c41a',
    },
  },
  {
    selector: 'node[type="edge"]',
    css: {
      'border-color': '#faad14',
      'color': '#faad14',
      'shape': 'diamond',
    },
  },
  {
    selector: 'node:selected',
    css: {
      'border-width': 4,
      'border-color': '#f5222d',
    },
  },
  {
    selector: 'edge',
    css: {
      'width': 2,
      'line-color': '#999',
      'target-arrow-color': '#999',
      'target-arrow-shape': 'triangle',
      'curve-style': 'bezier',
      'label': 'data(name)',
      'font-size': '12px',
      'color': '#666',
      'text-background-color': '#fff',
      'text-background-opacity': 1,
    },
  },
  {
    selector: 'edge[type="relationship"]',
    css: {
      'line-color': '#1890ff',
      'target-arrow-color': '#1890ff',
      'line-style': 'solid',
    },
  },
];

export default GraphCanvas;

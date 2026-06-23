import type cytoscape from 'cytoscape';
import type { GraphData, GraphStyleConfig } from '@/types/graph';

// Convert GraphData to Cytoscape elements
export function convertToCytoscapeElements(data: GraphData): cytoscape.ElementDefinition[] {
  const nodes = data.nodes.map((node) => ({
    data: {
      id: node.id,
      label: node.tag,
      ...node.properties,
      _tag: node.tag,
    },
  }));

  const edges = data.edges.map((edge) => ({
    data: {
      id: edge.id,
      source: edge.source,
      target: edge.target,
      label: edge.type,
      ...edge.properties,
      _type: edge.type,
      _rank: edge.rank,
    },
  }));

  return [...nodes, ...edges];
}

// Generate Cytoscape stylesheet
export function generateCytoscapeStyle(config: GraphStyleConfig): cytoscape.StylesheetCSS[] {
  const baseNodeStyle: cytoscape.Css.Node = {
    'background-color': '#666',
    'width': 40,
    'height': 40,
    'label': 'data(id)',
    'font-size': '12px',
    'text-valign': 'center',
    'text-halign': 'center',
    'color': '#333',
    'text-outline-color': '#fff',
    'text-outline-width': 1,
  };

  const baseEdgeStyle: cytoscape.Css.Edge = {
    'width': 2,
    'line-color': '#ccc',
    'curve-style': 'bezier',
    'target-arrow-shape': 'triangle',
    'target-arrow-color': '#ccc',
    'font-size': '10px',
    'color': '#666',
    'text-background-color': '#fff',
    'text-background-opacity': 0.8,
    'text-background-padding': '2px',
  };

  const nodeStyles = Object.entries(config.nodes).map(([tag, style]) => {
    const size = getNodeSize(style.size);
    return {
      selector: `node[_tag="${tag}"]`,
      css: {
        'background-color': style.color,
        'width': size,
        'height': size,
        'label': style.labelProperty === 'id' ? 'data(id)' : `data(${style.labelProperty})`,
      } as cytoscape.Css.Node,
    };
  });

  const edgeStyles = Object.entries(config.edges).map(([type, style]) => ({
    selector: `edge[_type="${type}"]`,
    css: {
      'line-color': style.color,
      'width': getEdgeWidth(style.width),
      'label': style.labelProperty === 'type' ? 'data(label)' : `data(${style.labelProperty})`,
      'target-arrow-color': style.color,
    } as cytoscape.Css.Edge,
  }));

  return [
    {
      selector: 'node',
      css: baseNodeStyle,
    },
    {
      selector: 'edge',
      css: baseEdgeStyle,
    },
    {
      selector: ':selected',
      css: {
        'border-width': 3,
        'border-color': '#1890ff',
        'border-opacity': 1,
      } as cytoscape.Css.Node,
    },
    ...nodeStyles,
    ...edgeStyles,
  ];
}

function getNodeSize(size: string): number {
  const sizes: Record<string, number> = { small: 30, medium: 40, large: 50 };
  return sizes[size] || 40;
}

function getEdgeWidth(width: string): number {
  const widths: Record<string, number> = { thin: 1, medium: 2, thick: 4 };
  return widths[width] || 2;
}

// Parse query result to graph data
export function parseQueryResultToGraph(data: unknown[]): GraphData {
  const nodes: GraphData['nodes'] = [];
  const edges: GraphData['edges'] = [];
  const nodeIds = new Set<string>();

  const processValue = (value: unknown) => {
    if (typeof value !== 'object' || value === null) return;

    const obj = value as Record<string, unknown>;

    // Check if it's a vertex/node
    if (obj.vid !== undefined) {
      const vid = String(obj.vid);
      if (!nodeIds.has(vid)) {
        nodeIds.add(vid);
        const tags = obj.tags as Record<string, Record<string, unknown>> | undefined;
        const firstTag = tags ? Object.keys(tags)[0] : 'unknown';
        nodes.push({
          id: vid,
          tag: firstTag,
          properties: tags?.[firstTag] || {},
        });
      }
    }

    // Check if it's an edge
    if (obj.srcID !== undefined && obj.dstID !== undefined && obj.edgeName !== undefined) {
      const srcId = String(obj.srcID);
      const dstId = String(obj.dstID);
      const edgeType = String(obj.edgeName);
      const rank = typeof obj.rank === 'number' ? obj.rank : 0;

      edges.push({
        id: `${edgeType}_${srcId}_${dstId}_${rank}`,
        type: edgeType,
        source: srcId,
        target: dstId,
        rank,
        properties: (obj.properties as Record<string, unknown>) || {},
      });

      // Add source and target nodes if not exists
      if (!nodeIds.has(srcId)) {
        nodeIds.add(srcId);
        nodes.push({
          id: srcId,
          tag: 'unknown',
          properties: {},
        });
      }
      if (!nodeIds.has(dstId)) {
        nodeIds.add(dstId);
        nodes.push({
          id: dstId,
          tag: 'unknown',
          properties: {},
        });
      }
    }
  };

  data.forEach((row) => {
    if (Array.isArray(row)) {
      row.forEach(processValue);
    } else {
      processValue(row);
    }
  });

  return { nodes, edges };
}

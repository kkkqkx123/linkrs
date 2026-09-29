import type cytoscape from 'cytoscape';
import type { GraphData, GraphStyleConfig } from '$types/graph';
import type { QueryResult } from '$types/query';

export const MAX_GRAPH_NODES = 800;
export const MAX_GRAPH_EDGES = 1200;

export interface GraphParseStats {
  skipped: number;
  completedUnknown: number;
  truncated: boolean;
}

export interface ParsedGraph extends GraphData {
  stats: GraphParseStats;
}

function stringifyId(value: unknown): string | null {
  if (value === null || value === undefined) return null;
  if (typeof value === 'string') return value;
  if (typeof value === 'number' || typeof value === 'bigint' || typeof value === 'boolean') return String(value);
  if (typeof value === 'object') {
    try {
      return JSON.stringify(value);
    } catch {
      return String(value);
    }
  }
  return String(value);
}

function asRecord(value: unknown): Record<string, unknown> | null {
  if (typeof value === 'object' && value !== null && !Array.isArray(value)) {
    return value as Record<string, unknown>;
  }
  return null;
}

function escapeEdgePart(value: string): string {
  return value.replace(/\\/g, '\\\\').replace(/\|/g, '\\|');
}

export function makeEdgeId(source: string, target: string, type: string, rank: number): string {
  return `e:${escapeEdgePart(type)}|${escapeEdgePart(source)}|${escapeEdgePart(target)}|${rank}`;
}

function extractVertex(value: unknown): GraphData['nodes'][number] | null {
  const obj = asRecord(value);
  if (!obj || obj.vid === undefined) return null;
  const vid = stringifyId(obj.vid);
  if (vid === null) return null;
  const tagObj = asRecord(obj.tag);
  let tag = 'unknown';
  let properties: Record<string, unknown> = {};
  if (tagObj && typeof tagObj.name === 'string') {
    tag = tagObj.name;
    const props = asRecord(tagObj.properties);
    if (props) properties = props;
  } else if (asRecord(obj.properties)) {
    properties = asRecord(obj.properties) as Record<string, unknown>;
    if (typeof obj.tag === 'string') tag = obj.tag;
  }
  return { id: vid, tag, properties };
}

function extractEdge(value: unknown): GraphData['edges'][number] | null {
  const obj = asRecord(value);
  if (!obj) return null;
  const srcRaw = obj.src ?? obj.srcID ?? obj.source;
  const dstRaw = obj.dst ?? obj.dstID ?? obj.target ?? obj.destination;
  const typeRaw = obj.edge_type ?? obj.edgeType ?? obj.edgeName ?? obj.type;
  if (srcRaw === undefined || dstRaw === undefined || typeRaw === undefined) return null;
  const source = stringifyId(srcRaw);
  const target = stringifyId(dstRaw);
  const type = String(typeRaw);
  if (source === null || target === null) return null;
  const rank = typeof obj.ranking === 'number' ? obj.ranking : typeof obj.rank === 'number' ? obj.rank : 0;
  const props = asRecord(obj.props) ?? asRecord(obj.properties) ?? {};
  return { id: makeEdgeId(source, target, type, rank), type, source, target, rank, properties: props };
}

function extractPath(value: unknown): { nodes: GraphData['nodes']; edges: GraphData['edges'] } | null {
  const obj = asRecord(value);
  if (!obj) return null;
  const vertices = obj.vertices;
  const edges = obj.edges;
  if (!Array.isArray(vertices) || !Array.isArray(edges)) return null;
  const nodes: GraphData['nodes'] = [];
  const parsedEdges: GraphData['edges'] = [];
  for (const v of vertices) {
    const node = extractVertex(v);
    if (node) nodes.push(node);
  }
  for (const e of edges) {
    const edge = extractEdge(e);
    if (edge) parsedEdges.push(edge);
  }
  if (nodes.length === 0 && parsedEdges.length === 0) return null;
  return { nodes, edges: parsedEdges };
}

export function convertToCytoscapeElements(data: GraphData, styles?: GraphStyleConfig): cytoscape.ElementDefinition[] {
  const nodes = data.nodes.map((node) => {
    const tagStyle = styles?.nodes[node.tag];
    const labelProp = tagStyle?.labelProperty;
    const label =
      labelProp && labelProp !== 'id' && node.properties[labelProp] !== undefined
        ? String(node.properties[labelProp])
        : node.id;
    return {
      data: {
        id: node.id,
        label,
        _tag: node.tag,
        props: node.properties,
      },
    };
  });
  const edges = data.edges.map((edge) => {
    const typeStyle = styles?.edges[edge.type];
    const labelProp = typeStyle?.labelProperty;
    const label =
      labelProp && labelProp !== 'type' && edge.properties[labelProp] !== undefined
        ? String(edge.properties[labelProp])
        : edge.type;
    return {
      data: {
        id: edge.id,
        source: edge.source,
        target: edge.target,
        label,
        _type: edge.type,
        _rank: edge.rank,
        props: edge.properties,
      },
    };
  });
  return [...nodes, ...edges];
}

function escapeSelector(value: string): string {
  return value.replace(/\\/g, '\\\\').replace(/"/g, '\\"');
}

export function generateCytoscapeStyle(config: GraphStyleConfig, dark = false): cytoscape.StylesheetCSS[] {
  const baseNodeStyle: cytoscape.Css.Node = {
    'background-color': '#666', 'width': 40, 'height': 40, 'label': 'data(id)',
    'font-size': '12px', 'text-valign': 'center', 'text-halign': 'center', 'color': dark ? '#e5e7eb' : '#333',
    'text-outline-color': dark ? '#111827' : '#fff', 'text-outline-width': 1,
  };
  const baseEdgeStyle: cytoscape.Css.Edge = {
    'width': 2, 'line-color': dark ? '#4b5563' : '#ccc', 'curve-style': 'bezier',
    'target-arrow-shape': 'triangle', 'target-arrow-color': dark ? '#4b5563' : '#ccc',
    'font-size': '10px', 'color': dark ? '#9ca3af' : '#666',
    'text-background-color': dark ? '#111827' : '#fff', 'text-background-opacity': 0.8, 'text-background-padding': '2px',
  };
  const nodeStyles = Object.entries(config.nodes).map(([tag, style]) => ({
    selector: `node[_tag="${escapeSelector(tag)}"]`,
    css: {
      'background-color': style.color,
      'width': getNodeSize(style.size),
      'height': getNodeSize(style.size),
      'label': style.labelProperty === 'id' ? 'data(id)' : `data(label)`,
    } as cytoscape.Css.Node,
  }));
  const edgeStyles = Object.entries(config.edges).map(([type, style]) => ({
    selector: `edge[_type="${escapeSelector(type)}"]`,
    css: {
      'line-color': style.color, 'width': getEdgeWidth(style.width),
      'label': 'data(label)',
      'target-arrow-color': style.color,
    } as cytoscape.Css.Edge,
  }));
  return [
    { selector: 'node', css: baseNodeStyle },
    { selector: 'edge', css: baseEdgeStyle },
    { selector: ':selected', css: { 'border-width': 3, 'border-color': '#1890ff', 'border-opacity': 1, 'line-color': '#1890ff', 'target-arrow-color': '#1890ff' } as cytoscape.Css.Node },
    ...nodeStyles, ...edgeStyles,
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

export function queryResultToGraph(result: QueryResult | null | undefined): ParsedGraph {
  const nodes: GraphData['nodes'] = [];
  const edges: GraphData['edges'] = [];
  const stats: GraphParseStats = { skipped: 0, completedUnknown: 0, truncated: false };
  if (!result || !result.rows) return { nodes, edges, stats };
  const nodeIds = new Set<string>();
  const edgeIds = new Set<string>();

  const addNode = (node: GraphData['nodes'][number]) => {
    if (nodeIds.has(node.id) || nodes.length >= MAX_GRAPH_NODES) {
      if (nodes.length >= MAX_GRAPH_NODES) stats.truncated = true;
      return;
    }
    nodeIds.add(node.id);
    nodes.push(node);
  };
  const addEdge = (edge: GraphData['edges'][number]) => {
    if (edgeIds.has(edge.id) || edges.length >= MAX_GRAPH_EDGES) {
      if (edges.length >= MAX_GRAPH_EDGES) stats.truncated = true;
      return;
    }
    edgeIds.add(edge.id);
    edges.push(edge);
    if (!nodeIds.has(edge.source)) {
      stats.completedUnknown += 1;
      addNode({ id: edge.source, tag: 'unknown', properties: {} });
    }
    if (!nodeIds.has(edge.target)) {
      stats.completedUnknown += 1;
      addNode({ id: edge.target, tag: 'unknown', properties: {} });
    }
  };

  const processCell = (cell: unknown) => {
    if (cell === null || cell === undefined) {
      stats.skipped += 1;
      return;
    }
    if (Array.isArray(cell)) {
      cell.forEach(processCell);
      return;
    }
    const path = extractPath(cell);
    if (path) {
      path.nodes.forEach(addNode);
      path.edges.forEach(addEdge);
      return;
    }
    const vertex = extractVertex(cell);
    if (vertex) {
      addNode(vertex);
      return;
    }
    const edge = extractEdge(cell);
    if (edge) {
      addEdge(edge);
      return;
    }
    stats.skipped += 1;
  };

  for (const row of result.rows) {
    if (stats.truncated) break;
    const record = asRecord(row);
    if (!record) {
      stats.skipped += 1;
      continue;
    }
    const columns = result.columns.length > 0 ? result.columns : Object.keys(record);
    for (const column of columns) {
      if (stats.truncated) break;
      processCell(record[column]);
    }
  }
  return { nodes, edges, stats };
}

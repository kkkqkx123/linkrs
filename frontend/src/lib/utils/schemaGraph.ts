import type { GraphData, GraphStyleConfig } from '$types/graph';
import type { Tag, EdgeType } from '$types/schema';

// Prefix applied to the synthetic `tag` value used by edge-type nodes. The
// double underscore keeps these internal identifiers from colliding with real
// tag names declared by users.
export const EDGE_TYPE_NODE_PREFIX = '__edge__';

// Distinct colors assigned to tag nodes, cycling when there are more tags than
// palette entries.
const TAG_PALETTE = [
  '#1890ff',
  '#52c41a',
  '#faad14',
  '#f5222d',
  '#722ed1',
  '#13c2c2',
  '#eb2f96',
  '#fa8c16',
];

const EDGE_TYPE_COLOR = '#8b5cf6';

export interface SchemaGraphResult {
  graph: GraphData;
  styles: GraphStyleConfig;
}

// Build a schema-level graph: tag definitions become nodes, edge-type
// definitions become standalone nodes. Edge types carry no instance endpoints
// in the schema, so they are rendered as independent nodes distinguished by the
// EDGE_TYPE_NODE_PREFIX marker rather than connections between tags.
export function buildSchemaGraph(tags: Tag[], edgeTypes: EdgeType[]): SchemaGraphResult {
  const nodes: GraphData['nodes'] = [];
  const styles: GraphStyleConfig = { nodes: {}, edges: {} };

  (tags ?? []).forEach((tag, index) => {
    if (!tag?.name) return;
    nodes.push({
      id: `tag:${tag.name}`,
      tag: tag.name,
      properties: {
        kind: 'tag',
        properties: (tag.properties ?? []).map((p) => `${p.name}: ${p.data_type}`).join(', '),
      },
    });
    styles.nodes[tag.name] = {
      color: TAG_PALETTE[index % TAG_PALETTE.length],
      size: 'medium',
      labelProperty: 'id',
    };
  });

  (edgeTypes ?? []).forEach((edge) => {
    if (!edge?.name) return;
    const syntheticTag = `${EDGE_TYPE_NODE_PREFIX}${edge.name}`;
    nodes.push({
      id: `edge:${edge.name}`,
      tag: syntheticTag,
      properties: {
        kind: 'edge',
        properties: (edge.properties ?? []).map((p) => `${p.name}: ${p.data_type}`).join(', '),
      },
    });
    styles.nodes[syntheticTag] = {
      color: EDGE_TYPE_COLOR,
      size: 'small',
      labelProperty: 'id',
    };
  });

  return { graph: { nodes, edges: [] }, styles };
}

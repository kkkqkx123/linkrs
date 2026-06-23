import type { Tag, EdgeType } from '@/types/schema';

export interface GraphNode {
  data: {
    id: string;
    type: 'tag' | 'edge';
    name: string;
    properties: Array<{ name: string; type: string }>;
    comment?: string;
    ttl?: {
      duration: number;
      col: string;
    };
    [key: string]: unknown;
  };
}

export interface GraphEdge {
  data: {
    id: string;
    source: string;
    target: string;
    type: 'relationship';
    name: string;
    [key: string]: unknown;
  };
}

export interface GraphData {
  elements: (GraphNode | GraphEdge)[];
}

export interface SampleData {
  edges: Array<{
    src: string;
    dst: string;
    name: string;
  }>;
  vidToTags: Record<string, string[]>;
}

export interface NodeData {
  id: string;
  type: 'tag' | 'edge';
  name: string;
  properties: Array<{ name: string; type: string }>;
  comment?: string;
  ttl?: {
    duration: number;
    col: string;
  };
}

/**
 * Build schema graph data
 *
 * Strategy:
 * 1. Each Tag as a node
 * 2. Each EdgeType as a node
 * 3. Infer relationships based on EdgeType name (simplified version)
 * 4. Or get real src/dst Tag relationships through sampled data
 */
export const buildSchemaGraph = (
  tags: Tag[],
  edgeTypes: EdgeType[],
  sampleData?: SampleData
): GraphData => {
  const elements: (GraphNode | GraphEdge)[] = [];

  // Add Tag nodes
  tags.forEach((tag) => {
    elements.push({
      data: {
        id: `tag_${tag.name}`,
        type: 'tag',
        name: tag.name,
        properties: tag.properties.map((p) => ({
          name: p.name,
          type: p.data_type,
        })),
        comment: tag.comment,
      },
    });
  });

  // Add Edge nodes
  edgeTypes.forEach((edge) => {
    elements.push({
      data: {
        id: `edge_${edge.name}`,
        type: 'edge',
        name: edge.name,
        properties: edge.properties.map((p) => ({
          name: p.name,
          type: p.data_type,
        })),
        comment: edge.comment,
      },
    });
  });

  // Add relationship edges
  // If sample data exists, use real relationships
  if (sampleData) {
    sampleData.edges.forEach((edge, index) => {
      const srcTags = sampleData.vidToTags[edge.src];
      const dstTags = sampleData.vidToTags[edge.dst];

      srcTags?.forEach((srcTag) => {
        dstTags?.forEach((dstTag) => {
          elements.push({
            data: {
              id: `rel_${index}_${srcTag}_${dstTag}_in`,
              source: `tag_${srcTag}`,
              target: `edge_${edge.name}`,
              type: 'relationship',
              name: '',
            },
          });
          elements.push({
            data: {
              id: `rel_${index}_${dstTag}_${srcTag}_out`,
              source: `edge_${edge.name}`,
              target: `tag_${dstTag}`,
              type: 'relationship',
              name: '',
            },
          });
        });
      });
    });
  } else {
    // Simplified: Edge connects to all Tags (indicating possible connections)
    edgeTypes.forEach((edge) => {
      tags.forEach((tag) => {
        elements.push({
          data: {
            id: `rel_${edge.name}_${tag.name}`,
            source: `edge_${edge.name}`,
            target: `tag_${tag.name}`,
            type: 'relationship',
            name: '',
          },
        });
      });
    });
  }

  return { elements };
};

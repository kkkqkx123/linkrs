import type cytoscape from 'cytoscape';
import type { LayoutType } from '@/types/graph';

export function applyLayout(cy: cytoscape.Core, layout: LayoutType): cytoscape.Layouts {
  const layouts: Record<LayoutType, cytoscape.LayoutOptions> = {
    force: {
      name: 'cose',
      padding: 30,
      nodeRepulsion: 4500,
      edgeElasticity: 100,
      gravity: 0.1,
      numIter: 2500,
      initialTemp: 200,
      coolingFactor: 0.95,
      minTemp: 1.0,
    },
    circle: {
      name: 'circle',
      padding: 30,
      fit: true,
    },
    grid: {
      name: 'grid',
      padding: 30,
      fit: true,
    },
    hierarchical: {
      name: 'breadthfirst',
      padding: 30,
      fit: true,
      directed: true,
      spacingFactor: 1.2,
    },
  };

  return cy.layout(layouts[layout]).run();
}

export function getLayoutOptions(): { label: string; value: LayoutType }[] {
  return [
    { label: 'Force Directed', value: 'force' },
    { label: 'Circle', value: 'circle' },
    { label: 'Grid', value: 'grid' },
    { label: 'Hierarchical', value: 'hierarchical' },
  ];
}

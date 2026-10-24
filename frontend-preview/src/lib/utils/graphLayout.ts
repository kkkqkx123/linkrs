import type cytoscape from 'cytoscape';
import type { LayoutType } from '$types/graph';
import { message } from '$i18n';

export interface LayoutParams {
	nodeRepulsion?: number;
	gravity?: number;
	numIter?: number;
}

export function applyLayout(
	cy: cytoscape.Core,
	layout: LayoutType,
	elementCount = 0,
	params?: LayoutParams,
): cytoscape.Layouts {
	const large = elementCount > 400;
	const layouts: Record<LayoutType, cytoscape.LayoutOptions> = {
		force: large
			? {
					name: 'cose',
					padding: 30,
					nodeRepulsion: params?.nodeRepulsion ?? 2000,
					edgeElasticity: 100,
					gravity: params?.gravity ?? 0.25,
					numIter: params?.numIter ?? 600,
					initialTemp: 120,
					coolingFactor: 0.95,
					minTemp: 1.0,
					animate: false,
					fit: true,
				}
			: {
					name: 'cose',
					padding: 30,
					nodeRepulsion: params?.nodeRepulsion ?? 4500,
					edgeElasticity: 100,
					gravity: params?.gravity ?? 0.1,
					numIter: params?.numIter ?? 1500,
					initialTemp: 200,
					coolingFactor: 0.95,
					minTemp: 1.0,
					animate: false,
					fit: true,
				},
		circle: { name: 'circle', padding: 30, fit: true },
		grid: { name: 'grid', padding: 30, fit: true },
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

export function getLayoutOptions(): {
	label: () => string;
	value: LayoutType;
}[] {
	return [
		{ label: message('graph.force'), value: 'force' },
		{ label: message('graph.circle'), value: 'circle' },
		{ label: message('graph.grid'), value: 'grid' },
		{ label: message('graph.hierarchical'), value: 'hierarchical' },
	];
}

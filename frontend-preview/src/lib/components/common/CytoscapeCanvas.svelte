<script lang="ts">
	import { onDestroy } from 'svelte';
	import type cytoscape from 'cytoscape';
	import {
		convertToCytoscapeElements,
		generateCytoscapeStyle,
	} from '$utils/cytoscapeConfig';
	import { applyLayout, type LayoutParams } from '$utils/graphLayout';
	import type { GraphData, GraphStyleConfig, LayoutType } from '$types/graph';

	interface NodeTapData {
		id: string;
		_tag?: string;
		label?: string;
		props?: Record<string, unknown>;
	}

	interface EdgeTapData {
		id: string;
		source: string;
		target: string;
		_type?: string;
		_rank?: number;
		props?: Record<string, unknown>;
	}

	let {
		data,
		styleConfig = { nodes: {}, edges: {} },
		layout = 'force',
		isDark = false,
		zoom = 1,
		relayoutToken = 0,
		layoutParams = { nodeRepulsion: 4500, gravity: 0.1, numIter: 1500 },
		cyInstance = $bindable<cytoscape.Core | null>(null),
		onNodeTap,
		onEdgeTap,
		onBackgroundTap,
		onZoom,
	}: {
		data: GraphData;
		styleConfig?: GraphStyleConfig;
		layout?: LayoutType;
		isDark?: boolean;
		zoom?: number;
		relayoutToken?: number;
		layoutParams?: LayoutParams;
		cyInstance?: cytoscape.Core | null;
		onNodeTap?: (data: NodeTapData) => void;
		onEdgeTap?: (data: EdgeTapData) => void;
		onBackgroundTap?: () => void;
		onZoom?: (zoom: number) => void;
	} = $props();

	let containerEl = $state<HTMLDivElement>();
	let initialized = $state(false);
	let eventsBound = $state(false);
	let lastRelayoutToken = 0;
	let layoutDuration = $state(0);
	let syncDuration = $state(0);

	export function getPerformanceMetrics() {
		return { layoutDuration, syncDuration };
	}

	function bindEvents(cy: cytoscape.Core) {
		if (eventsBound) return;
		cy.on('tap', 'node', (evt) => {
			onNodeTap?.(evt.target.data() as NodeTapData);
		});
		cy.on('tap', 'edge', (evt) => {
			onEdgeTap?.(evt.target.data() as EdgeTapData);
		});
		cy.on('tap', (evt) => {
			if (evt.target === cy) onBackgroundTap?.();
		});
		cy.on('zoom', () => {
			onZoom?.(cy.zoom());
		});
		eventsBound = true;
	}

	function refreshStyle() {
		if (!cyInstance) return;
		cyInstance.style(generateCytoscapeStyle(styleConfig, isDark));
		syncElements(false);
	}

	function syncElements(relayout: boolean) {
		if (!cyInstance || !data) return;
		const startSync = performance.now();
		const elements = convertToCytoscapeElements(data, styleConfig);
		const savedZoom = cyInstance.zoom();
		const savedPan = { ...cyInstance.pan() };
		const existingIds = new Set(cyInstance.elements().map((el) => el.id()));
		const nextIds = new Set(
			elements.map((el) => String((el.data as { id: string }).id)),
		);
		cyInstance.batch(() => {
			cyInstance
				?.elements()
				.filter((el) => !nextIds.has(el.id()))
				.remove();
			const toAdd = elements.filter(
				(el) => !existingIds.has(String((el.data as { id: string }).id)),
			);
			if (toAdd.length > 0) cyInstance?.add(toAdd);
			for (const el of elements) {
				const id = String((el.data as { id: string }).id);
				const existing = cyInstance?.getElementById(id);
				if (existing && existing.nonempty()) {
					const newData = el.data as Record<string, unknown>;
					const oldData = existing.data() as Record<string, unknown>;
					for (const key of Object.keys(newData)) {
						if (JSON.stringify(oldData[key]) !== JSON.stringify(newData[key])) {
							existing.data(key, newData[key]);
						}
					}
				}
			}
		});
		cyInstance.zoom(savedZoom);
		cyInstance.pan(savedPan);
		syncDuration = performance.now() - startSync;
		if (relayout) {
			const startLayout = performance.now();
			applyLayout(cyInstance, layout, cyInstance.elements().length, layoutParams);
			layoutDuration = performance.now() - startLayout;
		}
	}

	async function initCytoscape() {
		if (!containerEl || !data) return;
		try {
			const cytoscape = (await import('cytoscape')).default;
			if (cyInstance) {
				cyInstance.destroy();
				cyInstance = null;
				eventsBound = false;
			}
			const cy = cytoscape({
				container: containerEl,
				elements: convertToCytoscapeElements(data, styleConfig),
				style: generateCytoscapeStyle(styleConfig, isDark),
				layout: { name: 'preset' },
				minZoom: 0.1,
				maxZoom: 10,
				wheelSensitivity: 0.3,
			});
			bindEvents(cy);
			cyInstance = cy;
			initialized = true;
			if (zoom > 0 && zoom !== 1) cy.zoom(zoom);
		} catch (err) {
			initialized = false;
			throw err instanceof Error ? err : new Error('Cytoscape initialization failed');
		}
	}

	$effect(() => {
		if (containerEl && data && !initialized) {
			void initCytoscape();
		}
	});

	$effect(() => {
		if (!cyInstance) return;
		syncElements(false);
		if (relayoutToken !== lastRelayoutToken) {
			lastRelayoutToken = relayoutToken;
			applyLayout(cyInstance, layout, cyInstance.elements().length, layoutParams);
		} else if (layout) {
			applyLayout(cyInstance, layout, cyInstance.elements().length, layoutParams);
		}
	});

	$effect(() => {
		if (cyInstance) refreshStyle();
	});

	onDestroy(() => {
		if (cyInstance) {
			cyInstance.destroy();
			cyInstance = null;
		}
		initialized = false;
		eventsBound = false;
	});
</script>

<div
	class="absolute inset-0"
	style="min-height: 400px;"
	bind:this={containerEl}
></div>

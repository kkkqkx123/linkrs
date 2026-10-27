<script lang="ts">
	import { onDestroy } from 'svelte';
	import { computeWindow, estimateColumnWidths } from '$utils/virtualWindow';
	import { formatCellValue } from '$utils/parseData';

	export interface IndexedRow {
		index: number;
		row: Record<string, unknown>;
	}

	let {
		columns = [],
		rows = [],
		rowHeight = 40,
		height = 400,
	}: {
		columns: string[];
		rows: IndexedRow[];
		rowHeight?: number;
		height?: number;
	} = $props();

	let containerEl = $state<HTMLDivElement | undefined>(undefined);
	let scrollTop = $state(0);
	let viewportHeight = $derived(containerEl?.clientHeight ?? height);
	let raf = 0;

	let frozenKey = $state('');
	let widths = $state<number[]>([]);

	// Freeze column widths once the schema arrives so appended rows never
	// shift the layout while the stream is still flowing.
	$effect(() => {
		const cols = columns;
		if (cols.length === 0) return;
		const key = cols.join('\u0000');
		if (key === frozenKey) return;
		frozenKey = key;
		widths = estimateColumnWidths(
			cols,
			rows.slice(0, 50).map((entry) => entry.row),
			formatCellValue,
		);
	});

	let viewport = $derived(
		computeWindow(rows.length, scrollTop, viewportHeight, rowHeight, 1),
	);
	let visible = $derived(rows.slice(viewport.start, viewport.end));

	function onScroll() {
		const el = containerEl;
		if (!el || raf) return;
		const top = el.scrollTop;
		const client = el.clientHeight;
		raf = requestAnimationFrame(() => {
			raf = 0;
			scrollTop = top;
			viewportHeight = client;
		});
	}

	onDestroy(() => {
		if (raf) cancelAnimationFrame(raf);
	});
</script>

<div
	class="overflow-auto border border-line rounded"
	style="height: {height}px;"
	bind:this={containerEl}
	onscroll={onScroll}
>
	<table class="ui-table w-full border-collapse" style="table-layout: fixed;">
		<colgroup>
			<col style="width: 64px;" />
			{#each widths as width, i (i)}
				<col style="width: {width}px;" />
			{/each}
		</colgroup>
		<thead>
			<tr class="bg-secondary">
				<th
					class="px-3 py-2 text-left font-medium text-muted border-b border-line whitespace-nowrap sticky top-0 bg-secondary z-10"
					>#</th
				>
				{#each columns as column (column)}
					<th
						class="sticky top-0 bg-page z-10 overflow-hidden text-ellipsis"
						title={column}>{column}</th
					>
				{/each}
			</tr>
		</thead>
		<tbody>
			{#if viewport.topSpacer > 0}
				<tr
					><td
						colspan={columns.length + 1}
						style="height: {viewport.topSpacer}px; padding: 0; border: 0;"
					></td></tr
				>
			{/if}
			{#each visible as entry (entry.index)}
				<tr
					class="even:bg-hover"
					style="height: {rowHeight}px;"
				>
					<td class="text-muted whitespace-nowrap overflow-hidden"
						>{entry.index + 1}</td
					>
					{#each columns as column (column)}
						<td
							class="whitespace-nowrap overflow-hidden text-ellipsis"
							title={formatCellValue(entry.row[column])}
							>{formatCellValue(entry.row[column])}</td
						>
					{/each}
				</tr>
			{/each}
			{#if viewport.bottomSpacer > 0}
				<tr
					><td
						colspan={columns.length + 1}
						style="height: {viewport.bottomSpacer}px; padding: 0; border: 0;"
					></td></tr
				>
			{/if}
		</tbody>
	</table>
</div>

<script lang="ts">
	import { t } from '$i18n';
	import { Timer } from '@lucide/svelte';
	import EmptyState from '$components/common/EmptyState.svelte';
	import VirtualTable from '$components/common/VirtualTable.svelte';
	import { formatLatencyMs } from '$utils/metricsFormat';
	import type { QueryStatsResponse } from '$services/statistics';

	type SlowRow = NonNullable<QueryStatsResponse['slow_queries']>[number];
	type SortField = 'query' | 'duration' | 'trace';

	let {
		rows,
		slowMs,
		onselect,
	}: {
		rows: SlowRow[];
		slowMs: number;
		onselect: (traceId: string) => void;
	} = $props();

	let sortField = $state<SortField>('duration');
	let sortAsc = $state(false);

	function toggleSort(field: SortField) {
		if (sortField === field) {
			sortAsc = !sortAsc;
		} else {
			sortField = field;
			sortAsc = field !== 'duration';
		}
	}

	function sortIndicator(field: SortField): string {
		if (sortField !== field) return '↕';
		return sortAsc ? '↑' : '↓';
	}

	// Sorting happens client-side over the rows already loaded.
	const sortedRows = $derived.by(() => {
		const sorted = [...rows];
		sorted.sort((a, b) => {
			let cmp: number;
			if (sortField === 'duration') {
				cmp = (Number(a.duration_ms) || 0) - (Number(b.duration_ms) || 0);
			} else if (sortField === 'query') {
				cmp = String(a.query).localeCompare(String(b.query));
			} else {
				cmp = String(a.trace_id).localeCompare(String(b.trace_id));
			}
			return sortAsc ? cmp : -cmp;
		});
		return sorted;
	});

	// VirtualTable works on plain record rows; keep the original row reachable
	// through a single column so cell snippets can render it.
	const tableRows = $derived(
		sortedRows.map((row, index) => ({ index, row: { data: row } })),
	);
</script>

{#if rows.length === 0}
	<EmptyState
		icon={Timer}
		message={t('monitoring.noData')}
		hint={t('monitoring.slowQueries')}
	/>
{:else}
	<VirtualTable
		columns={['query', 'duration', 'trace']}
		rows={tableRows}
		rowHeight={32}
		height={400}
		rowClass={(row) => {
			const data = row.data as SlowRow;
			return Number(data.duration_ms) > slowMs ? 'bg-error-light' : '';
		}}
	>
		{#snippet header(column)}
			<button
				class="cursor-pointer select-none hover:underline"
				onclick={() => toggleSort(column as SortField)}
			>
				{column === 'query'
					? t('monitoring.query')
					: column === 'duration'
						? t('monitoring.duration')
						: t('monitoring.trace')}
				{sortIndicator(column as SortField)}
			</button>
		{/snippet}
		{#snippet cell(column, row)}
			{#if column === 'query'}
				<button
					class="hover:underline cursor-pointer text-left font-mono"
					onclick={() => onselect((row.data as SlowRow).trace_id)}
				>
					{(row.data as SlowRow).query}
				</button>
			{:else if column === 'duration'}
				<span class="font-mono">{formatLatencyMs((row.data as SlowRow).duration_ms)}</span>
			{:else}
				<span class="font-mono text-muted">{(row.data as SlowRow).trace_id}</span>
			{/if}
		{/snippet}
	</VirtualTable>
{/if}

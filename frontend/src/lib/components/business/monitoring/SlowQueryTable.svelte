<script lang="ts">
	import { t } from '$i18n';
	import { Timer } from '@lucide/svelte';
	import EmptyState from '$components/common/EmptyState.svelte';
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
</script>

<div class="overflow-x-auto">
	{#if rows.length === 0}
		<EmptyState
			icon={Timer}
			message={t('monitoring.noData')}
			hint={t('monitoring.slowQueries')}
		/>
	{:else}
		<table class="ui-table ui-table-dense min-w-full">
			<thead>
				<tr>
					<th>
						<button
							class="cursor-pointer select-none hover:underline"
							onclick={() => toggleSort('query')}
						>
							{t('monitoring.query')} {sortIndicator('query')}
						</button>
					</th>
					<th>
						<button
							class="cursor-pointer select-none hover:underline"
							onclick={() => toggleSort('duration')}
						>
							{t('monitoring.duration')} {sortIndicator('duration')}
						</button>
					</th>
					<th>
						<button
							class="cursor-pointer select-none hover:underline"
							onclick={() => toggleSort('trace')}
						>
							{t('monitoring.trace')} {sortIndicator('trace')}
						</button>
					</th>
				</tr>
			</thead>
			<tbody>
				{#each sortedRows as row (row.trace_id)}
					<tr class={Number(row.duration_ms) > slowMs ? 'bg-error-light' : ''}>
						<td class="max-w-80 truncate font-mono">
							<button
								class="hover:underline cursor-pointer text-left"
								onclick={() => onselect(row.trace_id)}
							>
								{row.query}
							</button>
						</td>
						<td class="font-mono">{formatLatencyMs(row.duration_ms)}</td>
						<td class="font-mono text-muted"
							>{row.trace_id}</td
						>
					</tr>
				{/each}
			</tbody>
		</table>
	{/if}
</div>

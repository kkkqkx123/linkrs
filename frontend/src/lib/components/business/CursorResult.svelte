<script lang="ts">
	import { t, type MessageKey } from '$i18n';
	import VirtualTable from '$components/common/VirtualTable.svelte';
	import EmptyState from '$components/common/EmptyState.svelte';
	import ResultViewTabs from '$components/business/workbench/result/ResultViewTabs.svelte';
	import GraphPreview from '$components/business/workbench/result/GraphPreview.svelte';
	import JsonPreview from '$components/business/workbench/result/JsonPreview.svelte';
	import ServerExportButtons from '$components/business/workbench/result/ServerExportButtons.svelte';
	import type { CursorState } from '$stores/cursor';
	import type { QueryResult } from '$types/query';
	import { formatRowCount } from '$utils/metricsFormat';
	import { queryResultToGraph } from '$utils/cytoscapeConfig';

	const GRAPH_PREFIX_LIMIT = 5000;

	let {
		cursor,
		activeView,
		isDark = false,
		onViewChange,
		onLoadMore,
		onClose,
		onOpenInGraph,
	}: {
		cursor: CursorState;
		activeView: 'table' | 'json' | 'graph';
		isDark?: boolean;
		onViewChange: (view: 'table' | 'json' | 'graph') => void;
		onLoadMore: () => void;
		onClose: () => void;
		onOpenInGraph: (result: QueryResult) => void;
	} = $props();

	let indexed = $derived.by(() => {
		const out: { index: number; row: Record<string, unknown> }[] = [];
		for (let i = 0; i < cursor.rows.length; i += 1) {
			out.push({ index: i, row: cursor.rows[i] });
		}
		return out;
	});

	let synthetic = $derived<QueryResult>({
		columns: cursor.columns,
		rows: cursor.rows,
		rowCount: cursor.receivedCount,
	});

	let graph = $derived.by(() => {
		if (activeView !== 'graph') return null;
		return queryResultToGraph({
			columns: cursor.columns,
			rows: cursor.rows.slice(0, GRAPH_PREFIX_LIMIT),
			rowCount: cursor.receivedCount,
		});
	});

	let loading = $derived(
		cursor.status === 'opening' || cursor.status === 'fetching',
	);

	let statusKey: MessageKey = $derived(
		cursor.status === 'opening'
			? 'console.cursorStatusOpening'
			: cursor.status === 'fetching'
				? 'console.streamStatusReceiving'
				: cursor.status === 'exhausted'
					? 'console.cursorStatusExhausted'
					: cursor.status === 'failed'
						? 'console.streamStatusFailed'
						: 'console.cursorStatusOpen',
	);
</script>

<div class="flex-1 ui-card ui-card-flush shadow-sm overflow-hidden flex flex-col">
<div
	class="ui-result-head"
>
	<span
		class="inline-block w-2 h-2 rounded-full {cursor.status === 'failed'
			? 'bg-error'
			: cursor.status === 'exhausted'
				? 'bg-success'
				: 'bg-info animate-pulse'}"
	></span>
	<span>{t(statusKey)}</span>
	<span>|</span>
	<span>{formatRowCount(cursor.receivedCount)}{cursor.hasMore ? '+' : ''}</span>
	{#if cursor.hasMore && cursor.status === 'open'}
		<button
			class="ml-2 ui-btn ui-btn-ghost ui-btn-sm"
			onclick={onLoadMore}
		>
			{t('console.cursorLoadMore')}
		</button>
	{/if}
	<button
		class="ui-btn ui-btn-ghost ui-btn-sm"
		onclick={onClose}
	>
		{t('common.close')}
	</button>
	<div class="flex-1"></div>
	<ResultViewTabs {activeView} {onViewChange} />
	<ServerExportButtons query={cursor.query} />
</div>

<div class="flex-1 overflow-auto p-5">
	{#if cursor.error}
		<div
			class="ui-alert ui-alert-error mb-3"
		>
			<p class="text-error font-medium text-sm">
				{cursor.error.code}
			</p>
			<p class="text-error text-sm mt-1">
				{cursor.error.message}
			</p>
		</div>
	{/if}
	{#if loading && cursor.rows.length === 0}
		<EmptyState message={t('common.loading')} />
	{:else if activeView === 'table'}
		{#if cursor.columns.length === 0}
			<EmptyState message={t('console.streamWaitingColumns')} />
		{:else}
			<VirtualTable columns={cursor.columns} rows={indexed} />
			{#if loading}
				<p class="mt-2 text-xs text-muted">{t('common.loading')}</p>
			{/if}
		{/if}
	{:else if activeView === 'json'}
		{#if cursor.rows.length === 0}
			<EmptyState message={t('console.noResult')} />
		{:else}
			<JsonPreview
				columns={cursor.columns}
				rows={cursor.rows}
				rowCount={cursor.receivedCount}
			/>
		{/if}
	{:else}
		<GraphPreview {graph} result={synthetic} {isDark} {onOpenInGraph} />
	{/if}
</div>
</div>

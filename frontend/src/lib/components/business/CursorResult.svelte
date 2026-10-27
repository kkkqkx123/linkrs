<script lang="ts">
	import { t, type MessageKey } from '$i18n';
	import { BarChart3, Link2, Braces } from 'lucide-svelte';
	import IconButton from '$components/common/IconButton.svelte';
	import VirtualTable from '$components/common/VirtualTable.svelte';
	import LazyCytoscapeCanvas from '$components/common/LazyCytoscapeCanvas.svelte';
	import GraphPreviewControls from '$components/business/GraphPreviewControls.svelte';
	import type { CursorState } from '$stores/cursor';
	import type { QueryResult } from '$types/query';
	import { formatRowCount } from '$utils/parseData';
	import { exportStreamViaServer } from '$utils/export';
	import { queryResultToGraph } from '$utils/cytoscapeConfig';
	import { STREAM_JSON_PREVIEW_LIMIT } from '$utils/virtualWindow';

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

	let previewActive = $state(false);
	const handlePreviewToggle = (active: boolean) => (previewActive = active);
	let exportError = $state<string | null>(null);

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

	let jsonPreview = $derived.by(() => {
		const shown = cursor.rows.slice(0, STREAM_JSON_PREVIEW_LIMIT);
		return JSON.stringify(
			{ columns: cursor.columns, rows: shown, rowCount: cursor.receivedCount },
			null,
			2,
		);
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

	async function handleServerExport(format: 'csv' | 'jsonl') {
		exportError = null;
		try {
			await exportStreamViaServer(cursor.query, format);
		} catch (error) {
			exportError =
				error instanceof Error
					? error.message
					: t('notification.serverExportFailed');
		}
	}
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
	<div class="flex gap-1">
		{#each ['table', 'json', 'graph'] as view (view)}
			<button
				class="ui-seg {activeView === view ? 'ui-seg-active' : ''}"
				onclick={() => onViewChange(view as 'table' | 'json' | 'graph')}
			>
				{#if view === 'table'}<BarChart3 size={14} /> {t('console.viewTable')}{:else if view === 'json'}<Braces size={14} /> {t('console.viewJson')}{:else}<Link2 size={14} /> {t('console.viewGraph')}{/if}
			</button>
		{/each}
	</div>
	<div class="h-4 w-px bg-line"></div>
	<button
		class="ui-link text-xs"
		title={t('console.serverExportHint')}
		onclick={() => void handleServerExport('csv')}>CSV</button
	>
	<button
		class="ui-link text-xs"
		title={t('console.serverExportHint')}
		onclick={() => void handleServerExport('jsonl')}>JSONL</button
	>
</div>

<div class="flex-1 overflow-auto p-5">
	{#if exportError}
		<div
			class="ui-alert ui-alert-error mb-3 flex items-start gap-2"
		>
			<p class="flex-1 text-error text-sm">{exportError}</p>
			<IconButton
				label={t('common.dismiss')}
				class="ui-link-danger text-sm"
				onclick={() => {
					exportError = null;
				}}
				size={14}
			/>
		</div>
	{/if}
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
		<div
			class="ui-empty"
		>
			{t('common.loading')}
		</div>
	{:else if activeView === 'table'}
		{#if cursor.columns.length === 0}
			<div
				class="ui-empty"
			>
				{t('console.streamWaitingColumns')}
			</div>
		{:else}
			<VirtualTable columns={cursor.columns} rows={indexed} />
			{#if loading}
				<p class="mt-2 text-xs text-muted">{t('common.loading')}</p>
			{/if}
		{/if}
	{:else if activeView === 'json'}
		{#if cursor.rows.length === 0}
			<div
				class="ui-empty"
			>
				{t('console.noResult')}
			</div>
		{:else}
			{#if cursor.receivedCount > STREAM_JSON_PREVIEW_LIMIT}
				<p class="mb-2 text-xs text-muted">
					{t('console.streamJsonTruncated', {
						shown: STREAM_JSON_PREVIEW_LIMIT,
						total: cursor.receivedCount,
					})}
				</p>
			{/if}
			<pre
				class="ui-pre ui-pre-lg">{jsonPreview}</pre>
		{/if}
	{:else}
		{#if graph && (graph.nodes.length > 0 || graph.edges.length > 0)}
			<div class="flex flex-col gap-2 text-sm text-muted">
				<GraphPreviewControls
					{graph}
					{previewActive}
					onOpenInGraph={() => onOpenInGraph(synthetic)}
					onTogglePreview={handlePreviewToggle}
				/>
				{#if previewActive}
					<div
						class="h-80 rounded border border-line relative overflow-hidden"
					>
						<LazyCytoscapeCanvas data={graph} {isDark} />
					</div>
				{/if}
			</div>
		{:else}
			<div class="ui-empty">
				{t('console.viewGraph')} - {t('graph.noData')}
			</div>
		{/if}
	{/if}
</div>
</div>

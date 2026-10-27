<script lang="ts">
	import { t, type MessageKey } from '$i18n';
	import { Clock, BarChart3, Link2, X, Check, Ban, Braces } from 'lucide-svelte';
	import IconButton from '$components/common/IconButton.svelte';
	import VirtualTable from '$components/common/VirtualTable.svelte';
	import LazyCytoscapeCanvas from '$components/common/LazyCytoscapeCanvas.svelte';
	import GraphPreviewControls from '$components/business/GraphPreviewControls.svelte';
	import type { StreamCardState, StreamState } from '$stores/stream';
	import type { QueryResult } from '$types/query';
	import { formatExecutionTime, formatRowCount } from '$utils/parseData';
	import { exportStreamViaServer } from '$utils/export';
	import { queryResultToGraph } from '$utils/cytoscapeConfig';
	import { STREAM_JSON_PREVIEW_LIMIT } from '$utils/virtualWindow';

	const GRAPH_PREFIX_LIMIT = 5000;

	let {
		stream,
		activeView,
		isDark = false,
		onViewChange,
		onCancel,
		onOpenInGraph,
	}: {
		stream: StreamState;
		activeView: 'table' | 'json' | 'graph';
		isDark?: boolean;
		onViewChange: (view: 'table' | 'json' | 'graph') => void;
		onCancel: () => void;
		onOpenInGraph: (result: QueryResult) => void;
	} = $props();

	let previewActive = $state(false);
	const handlePreviewToggle = (active: boolean) => (previewActive = active);
	let exportError = $state<string | null>(null);

	/** Server-side export re-executes the card's statement; failures leave no file. */
	async function handleServerExport(query: string, format: 'csv' | 'jsonl') {
		exportError = null;
		try {
			await exportStreamViaServer(query, format);
		} catch (error) {
			exportError =
				error instanceof Error
					? error.message
					: t('notification.serverExportFailed');
		}
	}

	interface CardModel {
		indexed: { index: number; row: Record<string, unknown> }[];
		dense: Record<string, unknown>[];
	}

	function cardModel(card: StreamCardState): CardModel {
		const indexed: { index: number; row: Record<string, unknown> }[] = [];
		const dense: Record<string, unknown>[] = [];
		const rows = card.rows;
		for (let i = 0; i < rows.length; i += 1) {
			const row = rows[i];
			if (row === undefined) continue;
			indexed.push({ index: i, row });
			dense.push(row);
		}
		return { indexed, dense };
	}

	function syntheticOf(
		card: StreamCardState,
		dense: Record<string, unknown>[],
	): QueryResult {
		return { columns: card.columns, rows: dense, rowCount: card.receivedCount };
	}

	function jsonPreviewOf(
		card: StreamCardState,
		dense: Record<string, unknown>[],
	): string {
		const shown = dense.slice(0, STREAM_JSON_PREVIEW_LIMIT);
		return JSON.stringify(
			{ columns: card.columns, rows: shown, rowCount: card.receivedCount },
			null,
			2,
		);
	}

	let totalRows = $derived(
		stream.cards.reduce((sum, card) => sum + card.receivedCount, 0),
	);

	let isActive = $derived(
		stream.status === 'connecting' || stream.status === 'receiving',
	);

	let statusKey: MessageKey = $derived(
		stream.status === 'connecting'
			? 'console.streamStatusConnecting'
			: stream.status === 'receiving'
				? 'console.streamStatusReceiving'
				: stream.status === 'completed'
					? 'console.streamStatusCompleted'
					: stream.status === 'cancelled'
						? 'console.streamStatusCancelled'
						: 'console.streamStatusFailed',
	);

	function cardStatusKey(status: StreamCardState['status']): MessageKey {
		if (status === 'pending') return 'console.streamCardPending';
		if (status === 'receiving') return 'console.streamStatusReceiving';
		if (status === 'completed') return 'console.streamStatusCompleted';
		if (status === 'cancelled') return 'console.streamStatusCancelled';
		return 'console.streamStatusFailed';
	}
</script>

{#snippet cardBody(card: StreamCardState)}
	{@const model = cardModel(card)}
	{@const synthetic = syntheticOf(card, model.dense)}
	{#if card.error}
		<div
			class="ui-alert ui-alert-error mb-3"
		>
			<p class="text-error font-medium text-sm">
				{card.error.code}
			</p>
			<p class="text-error text-sm mt-1">
				{card.error.message}
			</p>
			{#if card.error.code === 'ROW_LIMIT_EXCEEDED'}
				<p class="text-warning text-xs mt-1">
					{t('console.rowLimitHint')}
				</p>
			{/if}
		</div>
	{/if}
	{#if activeView === 'table'}
		{#if card.columns.length === 0}
			<div
				class="ui-empty"
			>
				{t('console.streamWaitingColumns')}
			</div>
		{:else}
			<VirtualTable columns={card.columns} rows={model.indexed} />
		{/if}
	{:else if activeView === 'json'}
		{#if model.dense.length === 0}
			<div
				class="ui-empty"
			>
				{t('console.noResult')}
			</div>
		{:else}
			{#if card.receivedCount > STREAM_JSON_PREVIEW_LIMIT}
				<p class="mb-2 text-xs text-muted">
					{t('console.streamJsonTruncated', {
						shown: STREAM_JSON_PREVIEW_LIMIT,
						total: card.receivedCount,
					})}
				</p>
			{/if}
			<pre
				class="ui-pre ui-pre-lg">{jsonPreviewOf(
					card,
					model.dense,
				)}</pre>
		{/if}
	{:else}
		{@const graph = queryResultToGraph({
			columns: card.columns,
			rows: model.dense.slice(0, GRAPH_PREFIX_LIMIT),
			rowCount: card.receivedCount,
		})}
		{#if graph.nodes.length > 0 || graph.edges.length > 0}
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
{/snippet}

<div class="flex-1 ui-card ui-card-flush shadow-sm overflow-hidden flex flex-col">
<div
	class="ui-result-head"
>
	<span
		class="inline-block w-2 h-2 rounded-full {stream.status === 'failed'
			? 'bg-error'
			: stream.status === 'cancelled'
				? 'bg-warning'
				: stream.status === 'completed'
					? 'bg-success'
					: 'bg-info animate-pulse'}"
	></span>
	<span>{t(statusKey)}</span>
	<span>|</span>
	{#if stream.executionTime > 0}
		<span
			><Clock size={12} /> {t('console.time')}: {formatExecutionTime(stream.executionTime)}</span
		>
		<span>|</span>
	{/if}
	<span>{formatRowCount(totalRows)}</span>
	{#if stream.batch}
		<span>|</span>
		<span>{t('console.streamBatchCount', { count: stream.cards.length })}</span>
	{/if}
	{#if isActive}
		<button
			class="ml-2 ui-btn ui-btn-danger-outline ui-btn-sm"
			onclick={onCancel}
		>
			{t('common.cancel')}
		</button>
	{/if}
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
	{#if !stream.batch && stream.cards.length === 1}
		{@const only = stream.cards[0]}
		<div class="h-4 w-px bg-line"></div>
		<button
			class="ui-link text-xs"
			title={t('console.serverExportHint')}
			onclick={() => void handleServerExport(only.query, 'csv')}>CSV</button
		>
		<button
			class="ui-link text-xs"
			title={t('console.serverExportHint')}
			onclick={() => void handleServerExport(only.query, 'jsonl')}>JSONL</button
		>
	{/if}
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
	{#if stream.error && !stream.batch}
		<div
			class="ui-alert ui-alert-error mb-3"
		>
			<p class="text-error font-medium text-sm">
				{stream.error.code}
			</p>
			<p class="text-error text-sm mt-1">
				{stream.error.message}
			</p>
		</div>
	{/if}
	{#if stream.batch}
		<div class="flex flex-col gap-3">
			{#each stream.cards as card (card.index)}
				<div
					class="border border-line rounded overflow-hidden"
				>
					<div
						class="px-3 py-2 bg-secondary flex items-center gap-2 text-xs text-muted"
					>
						<span
							class={card.status === 'failed'
								? 'text-error'
								: card.status === 'cancelled'
									? 'text-warning'
									: card.status === 'completed'
										? 'text-success'
										: 'text-info'}
							>{#if card.status === 'failed'}<X size={14} />{:else if card.status === 'completed'}<Check size={14} />{:else if card.status === 'cancelled'}<Ban size={14} />{:else}…{/if}</span
						>
						<span class="text-muted">#{card.index + 1}</span>
						<span
							class="font-mono truncate flex-1 text-ink"
							>{card.query}</span
						>
						<span>{t(cardStatusKey(card.status))}</span>
						{#if card.executionTime > 0}
							<span>{formatExecutionTime(card.executionTime)}</span>
						{/if}
						<span>{formatRowCount(card.receivedCount)}</span>
						<button
							class="ui-link text-xs"
							title={t('console.serverExportHint')}
							onclick={() => void handleServerExport(card.query, 'csv')}
							>CSV</button
						>
						<button
							class="ui-link text-xs"
							title={t('console.serverExportHint')}
							onclick={() => void handleServerExport(card.query, 'jsonl')}
							>JSONL</button
						>
					</div>
					<div class="p-3">
						{@render cardBody(card)}
					</div>
				</div>
			{/each}
		</div>
	{:else if stream.cards.length === 1}
		{@render cardBody(stream.cards[0])}
	{:else}
		<div
			class="ui-empty"
		>
			{t('console.noResult')}
		</div>
	{/if}
</div>
</div>

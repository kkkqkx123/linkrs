<script lang="ts">
	import { t, type MessageKey } from '$i18n';
	import { Clock, X, Check, Ban } from '@lucide/svelte';
	import VirtualTable from '$components/common/VirtualTable.svelte';
	import EmptyState from '$components/common/EmptyState.svelte';
	import ResultViewTabs from '$components/business/workbench/result/ResultViewTabs.svelte';
	import GraphPreview from '$components/business/workbench/result/GraphPreview.svelte';
	import JsonPreview from '$components/business/workbench/result/JsonPreview.svelte';
	import ServerExportButtons from '$components/business/workbench/result/ServerExportButtons.svelte';
	import type { StreamCardState, StreamState } from '$stores/stream';
	import type { QueryResult } from '$types/query';
	import { formatLatencyMs, formatRowCount } from '$utils/metricsFormat';
	import { queryResultToGraph } from '$utils/cytoscapeConfig';

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

	function graphOf(card: StreamCardState, dense: Record<string, unknown>[]) {
		return queryResultToGraph({
			columns: card.columns,
			rows: dense.slice(0, GRAPH_PREFIX_LIMIT),
			rowCount: card.receivedCount,
		});
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
			<EmptyState message={t('console.streamWaitingColumns')} />
		{:else}
			<VirtualTable columns={card.columns} rows={model.indexed} />
		{/if}
	{:else if activeView === 'json'}
		{#if model.dense.length === 0}
			<EmptyState message={t('console.noResult')} />
		{:else}
			<JsonPreview
				columns={card.columns}
				rows={model.dense}
				rowCount={card.receivedCount}
			/>
		{/if}
	{:else}
		{@const graph = graphOf(card, model.dense)}
		<GraphPreview {graph} result={synthetic} {isDark} {onOpenInGraph} />
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
			><Clock size={12} /> {t('console.time')}: {formatLatencyMs(stream.executionTime)}</span
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
	<ResultViewTabs {activeView} {onViewChange} />
	{#if !stream.batch && stream.cards.length === 1}
		<ServerExportButtons query={stream.cards[0].query} />
	{/if}
</div>

<div class="flex-1 overflow-auto p-5">
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
							<span>{formatLatencyMs(card.executionTime)}</span>
						{/if}
						<span>{formatRowCount(card.receivedCount)}</span>
						<ServerExportButtons query={card.query} />
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
		<EmptyState message={t('console.noResult')} />
	{/if}
</div>
</div>

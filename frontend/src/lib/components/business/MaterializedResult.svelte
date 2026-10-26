<script lang="ts">
	import { t } from '$i18n';
	import {
		formatExecutionTime,
		formatRowCount,
		formatCellValue,
	} from '$utils/parseData';
	import { queryResultToGraph } from '$utils/cytoscapeConfig';
	import { exportToCSV, exportToJSON } from '$utils/export';
	import LazyCytoscapeCanvas from '$components/common/LazyCytoscapeCanvas.svelte';
	import GraphPreviewControls from '$components/business/GraphPreviewControls.svelte';
	import type { QueryResult, QueryError } from '$types/query';
	import type { StatementResultEntry } from '$stores/console';

	interface Props {
		isExecuting: boolean;
		currentResult: QueryResult | null;
		results: StatementResultEntry[];
		executionTime: number;
		error: QueryError | null;
		activeView: 'table' | 'json' | 'graph';
		isDark: boolean;
		onViewChange: (view: 'table' | 'json' | 'graph') => void;
		onOpenInGraph: () => void;
		onJumpToError?: (position: { line: number; column: number }) => void;
	}

	let {
		isExecuting,
		currentResult,
		results,
		executionTime,
		error,
		activeView,
		isDark,
		onViewChange,
		onOpenInGraph,
		onJumpToError,
	}: Props = $props();

	let isMultiResult = $derived(results.length > 1);
	let primaryEntry = $derived(
		results.find((e) => e.success) ?? results[0] ?? null,
	);
	let graph = $derived.by(() => {
		if (!currentResult) return null;
		return queryResultToGraph(currentResult);
	});

	let previewActive = $state(false);

	function handlePreviewToggle(open: boolean) {
		previewActive = open;
	}

	function stageDetail(entry: StatementResultEntry | null): string {
		if (!entry?.stages) return '';
		const parts = Object.entries(entry.stages).map(([k, v]) => {
			const num = typeof v === 'number' ? v : Number(v);
			return `${k}: ${Number.isFinite(num) ? num.toFixed(2) : '?'}ms`;
		});
		const trace = entry.traceId ? ` | trace: ${entry.traceId}` : '';
		return parts.join(' | ') + trace;
	}
</script>

<div class="flex-1 ui-card ui-card-flush shadow-sm overflow-hidden flex flex-col">
	{#if isExecuting}
		<div class="ui-empty flex-1">
			<div class="text-center">
				<div
					class="ui-spinner ui-spinner-lg"
				></div>
				<p class="mt-2 text-sm text-gray-500 dark:text-gray-400">
					{t('common.loading')}
				</p>
			</div>
		</div>
	{:else if error && results.length === 0}
		<div
			class="ui-alert ui-alert-error m-5 p-5"
		>
			<p class="text-red-700 dark:text-red-400 font-medium text-sm">
				{error.code}
			</p>
			<p class="text-red-600 dark:text-red-300 text-sm mt-1">
				{error.message}
			</p>
			{#if error.position && onJumpToError}
				<button
					class="ui-btn ui-btn-danger-outline ui-btn-sm"
					onclick={() => onJumpToError?.(error.position!)}
				>
					{t('console.jumpToError', {
						line: error.position.line,
						column: error.position.column,
					})}
				</button>
			{/if}
		</div>
	{:else if isMultiResult || (results.length === 1 && !currentResult)}
		<div
			class="ui-result-head"
		>
			<span>⏱ {t('console.time')}: {formatExecutionTime(executionTime)}</span>
			<span>|</span>
			<span
				>{results.length}
				{t(
					results.length === 1 ? 'console.statement' : 'console.statements',
				)}</span
			>
		</div>
		<div class="flex-1 overflow-auto p-5 flex flex-col gap-3">
			{#each results as entry, index (index)}
				<details
					class="border border-gray-200 dark:border-gray-700 rounded overflow-hidden"
				>
					<summary
						class="px-3 py-2 bg-gray-50 dark:bg-gray-800/50 flex items-center gap-2 text-xs text-gray-500 dark:text-gray-400 cursor-pointer"
					>
						<span class={entry.success ? 'text-green-500' : 'text-red-500'}
							>{entry.success ? '✓' : '✗'}</span
						>
						<span class="text-gray-400">#{index + 1}</span>
						<span
							class="font-mono truncate flex-1 text-gray-700 dark:text-gray-300"
							>{entry.query}</span
						>
						<span title={entry.stages ? stageDetail(entry) : undefined}
							>{formatExecutionTime(entry.executionTime)}</span
						>
						{#if entry.traceId}
							<span class="font-mono text-gray-400" title={entry.traceId}
								>⛁ {entry.traceId.slice(0, 8)}</span
							>
						{/if}
						{#if entry.result}
							<span>{formatRowCount(entry.result.rowCount)}</span>
							{#if entry.truncated}
								<span
									class="text-amber-600 dark:text-amber-400"
									title={t('console.rowLimitHint')}
									>⚠ {t('console.resultTruncated')}</span
								>
							{/if}
						{/if}
					</summary>
					<div class="p-3">
						{#if !entry.success && entry.error}
							<div class="text-red-600 dark:text-red-400 text-xs">
								<span class="font-medium">{entry.error.code}</span>: {entry
									.error.message}
								{#if entry.error.position && onJumpToError}
									<button
										class="ui-btn ui-btn-danger-outline ui-btn-sm"
										onclick={() => onJumpToError?.(entry.error!.position!)}
									>
										{t('console.jumpToError', {
											line: entry.error!.position!.line,
											column: entry.error!.position!.column,
										})}
									</button>
								{/if}
							</div>
						{:else if entry.result}
							{#if entry.result.columns.length > 0}
								<div class="overflow-x-auto">
									<table class="ui-table w-full border-collapse">
										<thead>
											<tr class="bg-gray-50 dark:bg-gray-800/50">
												{#each entry.result.columns as col (col)}
													<th>{col}</th>
												{/each}
											</tr>
										</thead>
										<tbody>
											{#each entry.result.rows as row, i (i)}
												<tr class="even:bg-gray-50/50 dark:even:bg-gray-800/20">
													{#each entry.result.columns as col (col)}
														<td>{formatCellValue(row[col])}</td>
													{/each}
												</tr>
											{/each}
										</tbody>
									</table>
								</div>
							{:else}
								<div class="text-xs text-green-600 dark:text-green-400">
									{t('common.ok')}
								</div>
							{/if}
						{/if}
					</div>
				</details>
			{/each}
		</div>
	{:else if currentResult}
		<div
			class="ui-result-head"
		>
			<span
				title={primaryEntry?.stages
					? `${t('console.stages')}: ${stageDetail(primaryEntry)}`
					: undefined}
				>⏱ {t('console.time')}: {formatExecutionTime(executionTime)}</span
			>
			{#if primaryEntry?.traceId}
				<span
					class="font-mono text-xs text-gray-400"
					title={primaryEntry.traceId}
					>⛁ {primaryEntry.traceId.slice(0, 8)}</span
				>
			{/if}
			<span>|</span>
			<span>{formatRowCount(currentResult.rowCount)}</span>
			{#if currentResult.truncated}
				<span
					class="text-amber-600 dark:text-amber-400"
					title={t('console.rowLimitHint')}
					>⚠ {t('console.resultTruncated')}</span
				>
			{/if}
			<div class="flex-1"></div>
			<div class="flex gap-1">
				{#each ['table', 'json', 'graph'] as view (view)}
					<button
						class="ui-seg {activeView === view ? 'ui-seg-active' : ''}"
						onclick={() => onViewChange(view as 'table' | 'json' | 'graph')}
					>
						{view === 'table'
							? '📊 ' + t('console.viewTable')
							: view === 'json'
								? '{ } ' + t('console.viewJson')
								: '🔗 ' + t('console.viewGraph')}
					</button>
				{/each}
			</div>
			<div class="h-4 w-px bg-gray-300 dark:bg-gray-600"></div>
			<button
				class="ui-link text-xs"
				onclick={() => exportToCSV(currentResult)}>CSV</button
			>
			<button
				class="ui-link text-xs"
				onclick={() => exportToJSON(currentResult)}>JSON</button
			>
		</div>
		<div class="flex-1 overflow-auto p-5">
			{#if activeView === 'table'}
				<div class="overflow-x-auto">
					<table class="ui-table w-full border-collapse">
						<thead>
							<tr class="bg-gray-50 dark:bg-gray-800/50">
								{#each currentResult.columns as col (col)}
									<th>{col}</th>
								{/each}
							</tr>
						</thead>
						<tbody>
							{#each currentResult.rows as row, i (i)}
								<tr class="even:bg-gray-50/50 dark:even:bg-gray-800/20">
									{#each currentResult.columns as col (col)}
										<td>{formatCellValue(row[col])}</td>
									{/each}
								</tr>
							{/each}
						</tbody>
					</table>
				</div>
			{:else if activeView === 'json'}
				<pre
					class="ui-pre ui-pre-lg">{JSON.stringify(
						currentResult,
						null,
						2,
					)}</pre>
			{:else}
				{#if graph && (graph.nodes.length > 0 || graph.edges.length > 0)}
					<div
						class="flex flex-col gap-2 text-sm text-gray-600 dark:text-gray-300"
					>
						<GraphPreviewControls
							{graph}
							{previewActive}
							{onOpenInGraph}
							onTogglePreview={handlePreviewToggle}
						/>
						{#if previewActive && graph}
							<div
								class="h-80 rounded border border-gray-200 dark:border-gray-700 relative overflow-hidden"
							>
								<LazyCytoscapeCanvas data={graph} {isDark} />
							</div>
						{/if}
					</div>
				{:else}
					<div
						class="ui-empty"
					>
						{t('console.viewGraph')} - {t('graph.noData')}
					</div>
				{/if}
			{/if}
		</div>
	{:else}
		<div
			class="ui-empty flex-1"
		>
			{t('console.noResult')}
		</div>
	{/if}
</div>

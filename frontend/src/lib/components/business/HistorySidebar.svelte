<script lang="ts">
	import { t } from '$i18n';
	import type { QueryHistoryItem } from '$stores/console';

	interface Props {
		history: QueryHistoryItem[];
		onClose: () => void;
		onLoad: (item: QueryHistoryItem) => void;
		onRerun: (item: QueryHistoryItem) => void;
		onClear: () => void;
	}

	let { history, onClose, onLoad, onRerun, onClear }: Props = $props();
</script>

<div class="fixed inset-0 z-50 flex justify-end">
	<div class="absolute inset-0 bg-black/20" role="presentation" onclick={onClose}></div>
	<div
		class="relative w-96 bg-white dark:bg-[#1C2333] shadow-lg h-full overflow-y-auto"
		role="dialog"
		aria-labelledby="history-panel-title"
	>
		<div
			id="history-panel-title"
			class="p-4 border-b border-gray-200 dark:border-gray-700 flex items-center justify-between"
		>
			<h3 class="font-semibold text-gray-800 dark:text-gray-100">
				{t('console.history')}
			</h3>
			<button
				class="text-gray-400 hover:text-gray-600 dark:hover:text-gray-300 cursor-pointer text-lg"
				onclick={onClose}
				aria-label={t('common.close')}>✕</button
			>
		</div>
		<div class="p-4">
			{#if history.length === 0}
				<p class="text-gray-400 dark:text-gray-500 text-sm text-center py-4">
					{t('console.noResult')}
				</p>
			{:else}
				{#each history as item (item.id)}
					<div
						role="button"
						tabindex="0"
						class="mb-3 p-3 border border-gray-200 dark:border-gray-700 rounded hover:bg-gray-50 dark:hover:bg-gray-700/30 cursor-pointer"
						onclick={() => onLoad(item)}
						onkeydown={(e) => {
							if (e.key === 'Enter' || e.key === ' ') onLoad(item);
						}}
						title={t('console.historyLoadHint')}
					>
						<p class="text-xs font-mono text-gray-700 dark:text-gray-300 truncate mb-1">
							{item.query}
						</p>
						<div class="flex items-center gap-2 text-xs text-gray-400">
							<span class={item.success ? 'text-green-500' : 'text-red-500'}
								>{item.success ? '✓' : '✗'}</span
							>
							<span>{item.executionTime}ms</span>
							<span>{item.rowCount} {t('console.rows')}</span>
							{#if item.path}
								<span
									>· {t(
										item.path === 'stream'
											? 'console.historyViaStream'
											: 'console.historyViaMaterialized',
									)}</span
								>
							{/if}
							{#if item.streamStatus}
								<span
									>· {t(
										item.streamStatus === 'completed'
											? 'console.streamStatusCompleted'
											: item.streamStatus === 'cancelled'
												? 'console.streamStatusCancelled'
												: 'console.streamStatusFailed',
									)}</span
								>
							{/if}
							{#if item.path === 'stream' && item.reportedTotal !== undefined && item.reportedTotal !== null}
								<span
									>· {t('console.historyReceivedTotal', {
										received: item.receivedCount ?? item.rowCount,
										total: item.reportedTotal,
									})}</span
								>
							{/if}
							{#if item.errorCode}
								<span class="text-red-400">· {item.errorCode}</span>
							{/if}
							{#if item.traceId}
								<span class="font-mono" title={item.traceId}
									>· ⛁ {item.traceId.slice(0, 8)}</span
								>
							{/if}
						</div>
						<button
							class="mt-1 text-xs text-blue-500 dark:text-blue-400 hover:text-blue-700 dark:hover:text-blue-300 cursor-pointer"
							title={t('console.historyRerunHint')}
							onclick={(e) => {
								e.stopPropagation();
								onRerun(item);
							}}
						>
							↻ {t('console.historyRerun')}
						</button>
					</div>
				{/each}
				{#if history.length > 0}
					<button
						class="w-full text-center text-sm text-red-500 hover:text-red-700 py-2 cursor-pointer"
						onclick={onClear}
					>
						{t('common.delete')}
						{t('console.history')}
					</button>
				{/if}
			{/if}
		</div>
	</div>
</div>

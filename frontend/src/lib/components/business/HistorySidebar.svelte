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
	<div class="absolute inset-0 bg-mask" role="presentation" onclick={onClose}></div>
	<div
		class="ui-drawer"
		role="dialog"
		aria-labelledby="history-panel-title"
	>
		<div
			id="history-panel-title"
			class="p-5 border-b border-line flex items-center justify-between"
		>
			<h3 class="text-base font-semibold text-ink">
				{t('console.history')}
			</h3>
			<button
				class="ui-icon-btn text-lg"
				onclick={onClose}
				aria-label={t('common.close')}>✕</button
			>
		</div>
		<div class="p-4">
			{#if history.length === 0}
				<p class="text-muted text-sm text-center py-4">
					{t('console.noResult')}
				</p>
			{:else}
				{#each history as item (item.id)}
					<div
						role="button"
						tabindex="0"
						class="ui-list-item"
						onclick={() => onLoad(item)}
						onkeydown={(e) => {
							if (e.key === 'Enter' || e.key === ' ') onLoad(item);
						}}
						title={t('console.historyLoadHint')}
					>
						<p class="text-xs font-mono text-ink truncate mb-1">
							{item.query}
						</p>
						<div class="flex items-center gap-2 text-xs text-muted">
							<span class={item.success ? 'text-success' : 'text-error'}
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
								<span class="text-error">· {item.errorCode}</span>
							{/if}
							{#if item.traceId}
								<span class="font-mono" title={item.traceId}
									>· ⛁ {item.traceId.slice(0, 8)}</span
								>
							{/if}
						</div>
						<button
							class="mt-1 ui-link text-xs"
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
						class="w-full text-center text-sm ui-link-danger py-2"
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

<script lang="ts">
	import { t } from '$i18n';
	import type { QueryFavoriteItem } from '$stores/console';

	interface Props {
		favorites: QueryFavoriteItem[];
		onClose: () => void;
		onLoad: (fav: QueryFavoriteItem) => void;
		onRerun: (fav: QueryFavoriteItem) => void;
		onDelete: (id: string) => void;
	}

	let { favorites, onClose, onLoad, onRerun, onDelete }: Props = $props();
</script>

<div class="fixed inset-0 z-50 flex justify-end">
	<div class="absolute inset-0 bg-black/20" role="presentation" onclick={onClose}></div>
	<div
		class="ui-drawer"
		role="dialog"
		aria-labelledby="favorites-panel-title"
	>
		<div
			id="favorites-panel-title"
			class="p-5 border-b border-gray-200 dark:border-gray-700 flex items-center justify-between"
		>
			<h3 class="text-base font-semibold text-gray-800 dark:text-gray-100">
				{t('console.favorites')}
			</h3>
			<button
				class="ui-icon-btn text-lg"
				onclick={onClose}
				aria-label={t('common.close')}>✕</button
			>
		</div>
		<div class="p-4">
			{#if favorites.length === 0}
				<p class="text-gray-400 dark:text-gray-500 text-sm text-center py-4">
					{t('console.noResult')}
				</p>
			{:else}
				{#each favorites as fav (fav.id)}
					<div
						role="button"
						tabindex="0"
						class="ui-list-item"
						onclick={() => onLoad(fav)}
						onkeydown={(e) => {
							if (e.key === 'Enter' || e.key === ' ') onLoad(fav);
						}}
						title={t('console.historyLoadHint')}
					>
						<p class="text-sm font-medium text-gray-800 dark:text-gray-200 mb-1">
							{fav.name}
						</p>
						<p class="text-xs font-mono text-gray-500 dark:text-gray-400 truncate">
							{fav.query}
						</p>
						{#if fav.preferredPath}
							<p class="text-xs text-gray-400 mt-1">
								{t('console.favoriteSavedPath', {
									path: t(
										fav.preferredPath === 'stream'
											? 'console.execStream'
											: fav.preferredPath === 'auto'
												? 'console.execAuto'
												: 'console.execMaterialized',
									),
								})}
							</p>
						{/if}
						<div class="flex gap-3">
							<button
								class="mt-1 ui-link text-xs"
								title={t('console.historyRerunHint')}
								onclick={(e) => {
									e.stopPropagation();
									onRerun(fav);
								}}
							>
								↻ {t('console.historyRerun')}
							</button>
							<button
								class="mt-1 text-xs ui-link-danger"
								onclick={(e) => {
									e.stopPropagation();
									onDelete(fav.id);
								}}
							>
								{t('common.delete')}
							</button>
						</div>
					</div>
				{/each}
			{/if}
		</div>
	</div>
</div>

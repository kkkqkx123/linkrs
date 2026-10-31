<script lang="ts">
	import { t } from '$i18n';
	import { RotateCw } from '@lucide/svelte';
	import Drawer from '$components/common/Drawer.svelte';
	import EmptyState from '$components/common/EmptyState.svelte';
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

<Drawer title={t('console.favorites')} {onClose}>
	{#if favorites.length === 0}
		<EmptyState message={t('console.noResult')} />
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
				<p class="text-sm font-medium text-ink mb-1">
					{fav.name}
				</p>
				<p class="text-xs font-mono text-muted truncate">
					{fav.query}
				</p>
				{#if fav.preferredPath}
					<p class="text-xs text-muted mt-1">
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
						<RotateCw size={12} /> {t('console.historyRerun')}
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
</Drawer>

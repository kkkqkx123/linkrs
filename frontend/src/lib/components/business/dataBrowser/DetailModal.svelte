<script lang="ts">
	import { t } from '$i18n';
	import IconButton from '$components/common/IconButton.svelte';
	import { dataBrowserStore } from '$stores/dataBrowser';
	import type { VertexData, EdgeData } from '$types/dataBrowser';

	let {
		detailData,
		detailType,
	}: {
		detailData: VertexData | EdgeData;
		detailType: string | null;
	} = $props();
</script>

<div class="fixed inset-0 z-50 flex items-center justify-center">
	<div
		role="presentation"
		class="absolute inset-0 bg-mask"
		onclick={() => dataBrowserStore.hideDetail()}
	></div>
	<div
		class="relative ui-card shadow-lg p-6 w-96 max-h-[80vh] overflow-y-auto"
	>
		<div class="flex items-center justify-between mb-4">
			<h2 class="text-base font-semibold text-ink">
				{detailType === 'vertex'
					? t('dataBrowser.vertices')
					: t('sidebar.edges')}
				{t('common.detail')}
			</h2>
			<IconButton
				onclick={() => dataBrowserStore.hideDetail()}
				label={t('common.close')}
				class="ui-icon-btn"
				size={18}
			/>
		</div>
		<div class="space-y-2">
			{#each Object.entries(detailData) as [key, value] (key)}
				{#if key !== 'properties'}
					<div class="text-sm">
						<span class="text-muted">{key}:</span>
						<span class="ml-1 text-ink"
							>{String(value)}</span
						>
					</div>
				{/if}
			{/each}
			{#if detailData.properties}
				<div class="mt-4">
					<h3 class="text-sm font-medium text-ink mb-2">
						{t('common.properties')}
					</h3>
					{#each Object.entries(detailData.properties) as [k, v] (k)}
						<div class="text-sm ml-2">
							<span class="text-muted">{k}:</span>
							<span class="ml-1 text-ink"
								>{String(v)}</span
							>
						</div>
					{/each}
				</div>
			{/if}
		</div>
	</div>
</div>

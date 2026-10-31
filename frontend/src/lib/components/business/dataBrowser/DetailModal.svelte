<script lang="ts">
	import { t } from '$i18n';
	import Modal from '$components/common/Modal.svelte';
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

<Modal
	title="{detailType === 'vertex'
		? t('dataBrowser.vertices')
		: t('sidebar.edges')} {t('common.detail')}"
	onClose={() => dataBrowserStore.hideDetail()}
>
	<div class="space-y-2">
		{#each Object.entries(detailData) as [key, value] (key)}
			{#if key !== 'properties'}
				<div class="text-sm">
					<span class="text-muted">{key}:</span>
					<span class="ml-1 text-ink">{String(value)}</span>
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
						<span class="ml-1 text-ink">{String(v)}</span>
					</div>
				{/each}
			</div>
		{/if}
	</div>
</Modal>

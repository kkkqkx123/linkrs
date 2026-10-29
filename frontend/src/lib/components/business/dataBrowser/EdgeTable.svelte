<script lang="ts">
	import { t } from '$i18n';
	import { ArrowLeftRight } from '@lucide/svelte';
	import EmptyState from '$components/common/EmptyState.svelte';
	import { formatCellValue } from '$utils/parseData';
	import type { EdgeData } from '$types/dataBrowser';
	import type { EdgeType } from '$types/schema';

	let {
		edgeTypes,
		selectedEdgeType,
		loading,
		edges,
		properties,
		total,
		page,
		pageSize,
		selectedIds,
		onEdgeTypeChange,
		onToggleSelect,
		onViewDetail,
		onViewInGraph,
		onCopyId,
		onEdit,
		onDelete,
		onPrevPage,
		onNextPage,
	}: {
		edgeTypes: EdgeType[];
		selectedEdgeType: string | null;
		loading: boolean;
		edges: EdgeData[];
		properties: string[];
		total: number;
		page: number;
		pageSize: number;
		selectedIds: Set<string>;
		onEdgeTypeChange: (e: Event) => void;
		onToggleSelect: (id: string) => void;
		onViewDetail: (e: EdgeData) => void;
		onViewInGraph: (e: EdgeData) => void;
		onCopyId: (id: string) => void;
		onEdit: (e: EdgeData) => void;
		onDelete: (e: EdgeData) => void;
		onPrevPage: () => void;
		onNextPage: () => void;
	} = $props();
</script>

<div class="mb-4">
	<select
		class="ui-input"
		value={selectedEdgeType || ''}
		onchange={onEdgeTypeChange}
	>
		<option value="">-- {t('dataBrowser.selectEdgeType')} --</option>
		{#each edgeTypes as et (et.name)}
			<option value={et.name}>{et.name}</option>
		{/each}
	</select>
</div>

{#if loading}
	<div class="ui-empty">
		<div class="ui-spinner"></div>
	</div>
{:else if edges.length > 0}
	<div class="overflow-x-auto">
		<table class="ui-table w-full border-collapse">
			<thead>
				<tr class="bg-secondary">
					<th>{t('dataBrowser.batch.deletedColumn')}</th>
					<th>ID</th>
					<th>{t('common.type')}</th>
					<th>{t('dataBrowser.source')}</th>
					<th>{t('dataBrowser.target')}</th>
					<th>{t('dataBrowser.rank')}</th>
					{#each properties as prop (prop)}
						<th>{prop}</th>
					{/each}
					<th>{t('common.actions')}</th>
				</tr>
			</thead>
			<tbody>
				{#each edges as e (e.id)}
					<tr>
						<td>
							<input
								type="checkbox"
								class="cursor-pointer"
								checked={selectedIds.has(String(e.id))}
								onchange={() => onToggleSelect(String(e.id))}
							/>
						</td>
						<td class="font-mono">{e.id}</td>
						<td>{e.type}</td>
						<td class="font-mono">{e.src}</td>
						<td class="font-mono">{e.dst}</td>
						<td>{e.rank}</td>
						{#each properties as prop (prop)}
							<td class="max-w-40 truncate"
								>{formatCellValue(e.properties[prop])}</td
							>
						{/each}
						<td>
							<button
								class="ui-link text-xs"
								onclick={() => onViewDetail(e)}
								>{t('dataBrowser.viewDetail')}</button
							>
							<button
								class="ml-2 text-success hover:text-success text-xs cursor-pointer"
								onclick={() => onViewInGraph(e)}
								>{t('graphPreview.openInGraph')}</button
							>
							<button
								class="ml-2 text-muted hover:text-ink text-xs cursor-pointer"
								onclick={() => onCopyId(String(e.src))}
								>{t('dataBrowser.copyId')}</button
							>
							<button
								class="ml-2 text-warning hover:text-warning text-xs cursor-pointer"
								onclick={() => onEdit(e)}
								>{t('dataBrowser.editUpdate')}</button
							>
							<button
								class="ml-2 ui-link-danger text-xs"
								onclick={() => onDelete(e)}
								>{t('dataBrowser.editDelete')}</button
							>
						</td>
					</tr>
				{/each}
			</tbody>
		</table>
	</div>
	<div class="flex items-center justify-between mt-4 text-sm text-muted">
		<span>{t('dataBrowser.total')}: {total} {t('dataBrowser.items')}</span>
		<div class="flex gap-2">
			<button
				class="ui-btn ui-btn-ghost ui-btn-sm"
				disabled={page <= 1}
				onclick={onPrevPage}>{t('common.prev')}</button
			>
			<span class="px-3 py-1 text-muted">{t('common.page')} {page}</span>
			<button
				class="ui-btn ui-btn-ghost ui-btn-sm"
				disabled={page * pageSize >= total}
				onclick={onNextPage}>{t('common.next')}</button
			>
		</div>
	</div>
{:else if selectedEdgeType}
	<EmptyState
		icon={ArrowLeftRight}
		message={t('schema.noEdges')}
		hint={t('dataBrowser.emptyHint')}
	/>
{:else}
	<EmptyState icon={ArrowLeftRight} message={t('dataBrowser.selectEdgeType')} />
{/if}

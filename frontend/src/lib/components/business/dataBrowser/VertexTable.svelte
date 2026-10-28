<script lang="ts">
	import { t } from '$i18n';
	import { Table } from 'lucide-svelte';
	import EmptyState from '$components/common/EmptyState.svelte';
	import { formatCellValue } from '$utils/parseData';
	import type { VertexData } from '$types/dataBrowser';
	import type { Tag } from '$types/schema';

	let {
		tags,
		selectedTag,
		loading,
		vertices,
		properties,
		total,
		page,
		pageSize,
		selectedIds,
		onTagChange,
		onToggleSelect,
		onViewDetail,
		onViewInGraph,
		onCopyId,
		onEdit,
		onDelete,
		onPrevPage,
		onNextPage,
	}: {
		tags: Tag[];
		selectedTag: string | null;
		loading: boolean;
		vertices: VertexData[];
		properties: string[];
		total: number;
		page: number;
		pageSize: number;
		selectedIds: Set<string>;
		onTagChange: (e: Event) => void;
		onToggleSelect: (id: string) => void;
		onViewDetail: (v: VertexData) => void;
		onViewInGraph: (v: VertexData) => void;
		onCopyId: (id: string) => void;
		onEdit: (v: VertexData) => void;
		onDelete: (v: VertexData) => void;
		onPrevPage: () => void;
		onNextPage: () => void;
	} = $props();
</script>

<div class="mb-4">
	<select class="ui-input" value={selectedTag || ''} onchange={onTagChange}>
		<option value="">-- {t('dataBrowser.selectTag')} --</option>
		{#each tags as tag (tag.name)}
			<option value={tag.name}>{tag.name}</option>
		{/each}
	</select>
</div>

{#if loading}
	<div class="ui-empty">
		<div class="ui-spinner"></div>
	</div>
{:else if vertices.length > 0}
	<div class="overflow-x-auto">
		<table class="ui-table w-full border-collapse">
			<thead>
				<tr class="bg-secondary">
					<th>{t('dataBrowser.batch.deletedColumn')}</th>
					<th>ID</th>
					<th>{t('sidebar.tags')}</th>
					{#each properties as prop (prop)}
						<th>{prop}</th>
					{/each}
					<th>{t('common.actions')}</th>
				</tr>
			</thead>
			<tbody>
				{#each vertices as v (v.id)}
					<tr>
						<td>
							<input
								type="checkbox"
								class="cursor-pointer"
								checked={selectedIds.has(String(v.id))}
								onchange={() => onToggleSelect(String(v.id))}
							/>
						</td>
						<td class="font-mono">{v.id}</td>
						<td>{v.tag}</td>
						{#each properties as prop (prop)}
							<td class="max-w-40 truncate"
								>{formatCellValue(v.properties[prop])}</td
							>
						{/each}
						<td>
							<button
								class="ui-link text-xs"
								onclick={() => onViewDetail(v)}
								>{t('dataBrowser.viewDetail')}</button
							>
							<button
								class="ml-2 text-success hover:text-success text-xs cursor-pointer"
								onclick={() => onViewInGraph(v)}
								>{t('graphPreview.openInGraph')}</button
							>
							<button
								class="ml-2 text-muted hover:text-ink text-xs cursor-pointer"
								onclick={() => onCopyId(String(v.id))}
								>{t('dataBrowser.copyId')}</button
							>
							<button
								class="ml-2 text-warning hover:text-warning text-xs cursor-pointer"
								onclick={() => onEdit(v)}
								>{t('dataBrowser.editUpdate')}</button
							>
							<button
								class="ml-2 ui-link-danger text-xs"
								onclick={() => onDelete(v)}
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
{:else if selectedTag}
	<EmptyState
		icon={Table}
		message={t('schema.noTags')}
		hint={t('dataBrowser.emptyHint')}
	/>
{:else}
	<EmptyState icon={Table} message={t('dataBrowser.selectTag')} />
{/if}

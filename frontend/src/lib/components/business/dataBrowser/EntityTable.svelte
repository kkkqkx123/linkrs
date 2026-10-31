<script lang="ts" generics="T extends { id: string; properties: Record<string, unknown> }">
	import { t } from '$i18n';
	import type { LucideIcon } from '@lucide/svelte';
	import EmptyState from '$components/common/EmptyState.svelte';
	import LoadingState from '$components/common/LoadingState.svelte';
	import { formatCellValue } from '$utils/parseData';

	/** Extra entity columns rendered between the ID column and the properties. */
	interface Column {
		header: string;
		value: (row: T) => string | number;
		mono?: boolean;
	}

	let {
		filterOptions,
		filterLabel,
		selectedFilter,
		onFilterChange,
		loading,
		rows,
		columns,
		properties,
		total,
		page,
		pageSize,
		selectedIds,
		icon,
		emptyNoSelection,
		emptyFiltered,
		emptyHint,
		onToggleSelect,
		onViewDetail,
		onViewInGraph,
		onCopyId,
		onEdit,
		onDelete,
		onPrevPage,
		onNextPage,
	}: {
		filterOptions: string[];
		filterLabel: string;
		selectedFilter: string | null;
		onFilterChange: (e: Event) => void;
		loading: boolean;
		rows: T[];
		columns: Column[];
		properties: string[];
		total: number;
		page: number;
		pageSize: number;
		selectedIds: Set<string>;
		icon: LucideIcon;
		emptyNoSelection: string;
		emptyFiltered: string;
		emptyHint: string;
		onToggleSelect: (id: string) => void;
		onViewDetail: (row: T) => void;
		onViewInGraph: (row: T) => void;
		onCopyId: (row: T) => void;
		onEdit: (row: T) => void;
		onDelete: (row: T) => void;
		onPrevPage: () => void;
		onNextPage: () => void;
	} = $props();
</script>

<div class="mb-4">
	<select class="ui-input" value={selectedFilter || ''} onchange={onFilterChange}>
		<option value="">-- {filterLabel} --</option>
		{#each filterOptions as opt (opt)}
			<option value={opt}>{opt}</option>
		{/each}
	</select>
</div>

{#if loading}
	<LoadingState />
{:else if rows.length > 0}
	<div class="overflow-x-auto">
		<table class="ui-table w-full border-collapse">
			<thead>
				<tr class="bg-secondary">
					<th>{t('dataBrowser.batch.deletedColumn')}</th>
					<th>ID</th>
					{#each columns as col (col.header)}
						<th>{col.header}</th>
					{/each}
					{#each properties as prop (prop)}
						<th>{prop}</th>
					{/each}
					<th>{t('common.actions')}</th>
				</tr>
			</thead>
			<tbody>
				{#each rows as row (row.id)}
					<tr>
						<td>
							<input
								type="checkbox"
								class="cursor-pointer"
								checked={selectedIds.has(String(row.id))}
								onchange={() => onToggleSelect(String(row.id))}
							/>
						</td>
						<td class="font-mono">{row.id}</td>
						{#each columns as col (col.header)}
							<td class={col.mono ? 'font-mono' : ''}>{col.value(row)}</td>
						{/each}
						{#each properties as prop (prop)}
							<td class="max-w-40 truncate"
								>{formatCellValue(row.properties[prop])}</td
							>
						{/each}
						<td>
							<button
								class="ui-link text-xs"
								onclick={() => onViewDetail(row)}
								>{t('dataBrowser.viewDetail')}</button
							>
							<button
								class="ml-2 text-success hover:text-success text-xs cursor-pointer"
								onclick={() => onViewInGraph(row)}
								>{t('graphPreview.openInGraph')}</button
							>
							<button
								class="ml-2 text-muted hover:text-ink text-xs cursor-pointer"
								onclick={() => onCopyId(row)}
								>{t('dataBrowser.copyId')}</button
							>
							<button
								class="ml-2 text-warning hover:text-warning text-xs cursor-pointer"
								onclick={() => onEdit(row)}
								>{t('dataBrowser.editUpdate')}</button
							>
							<button
								class="ml-2 ui-link-danger text-xs"
								onclick={() => onDelete(row)}
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
{:else if selectedFilter}
	<EmptyState icon={icon} message={emptyFiltered} hint={emptyHint} />
{:else}
	<EmptyState icon={icon} message={emptyNoSelection} />
{/if}

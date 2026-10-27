<script lang="ts">
	import { onMount } from 'svelte';
	import { t } from '$i18n';
	import { RefreshCw, Search, Package, ArrowLeftRight, X } from 'lucide-svelte';
	import IconButton from '$components/common/IconButton.svelte';
	import { get } from 'svelte/store';
	import { goto } from '$app/navigation';
	import { SvelteSet } from 'svelte/reactivity';
	import { dataBrowserStore } from '$stores/dataBrowser';
	import { graphStore } from '$stores/graph';
	import { schemaStore } from '$stores/schema';
	import PageHeader from '$components/layout/PageHeader.svelte';
	import { dataBrowserService } from '$services/dataBrowser';
	import { formatCellValue } from '$utils/parseData';
	import {
		buildEdgeDelete,
		buildEdgeUpdate,
		buildVertexDelete,
		buildVertexUpdate,
	} from '$utils/cypherTemplates';
	import PageSkeleton from '$components/common/PageSkeleton.svelte';
	import FilterPanel from '$components/business/FilterPanel.svelte';
	import VectorSearchPanel from '$components/business/VectorSearchPanel.svelte';
	import FulltextSearchPanel from '$components/business/FulltextSearchPanel.svelte';
	import type { DataBrowserTab } from '$types/dataBrowser';
	import EditPreviewModal from '$components/business/EditPreviewModal.svelte';
	import type { VertexData, EdgeData, Statistics } from '$types/dataBrowser';
	import type { Tag, EdgeType } from '$types/schema';
	import { queryService } from '$services/query';

	let currentSpace = $state<string | null>(null);
	let tags = $state<Tag[]>([]);
	let edgeTypes = $state<EdgeType[]>([]);

	let activeTab = $state<DataBrowserTab>('vertices');
	let selectedTag = $state<string | null>(null);
	let selectedEdgeType = $state<string | null>(null);
	let vertices = $state<VertexData[]>([]);
	let edges = $state<EdgeData[]>([]);
	let vertexTotal = $state(0);
	let edgeTotal = $state(0);
	let vertexPage = $state(1);
	let edgePage = $state(1);
	let vertexPageSize = $state(50);
	let edgePageSize = $state(50);
	let loading = $state(false);
	let error = $state<string | null>(null);
	let statistics = $state<Statistics | null>(null);
	let filterPanelVisible = $state(false);
	let detailModalVisible = $state(false);
	let detailData = $state<VertexData | EdgeData | null>(null);
	let detailType = $state<string | null>(null);

	let vertexProperties = $state<string[]>([]);
	let edgeProperties = $state<string[]>([]);
	let pageInitialized = $state(false);

	onMount(() => {
		const unsub1 = schemaStore.subscribe((s) => {
			currentSpace = s.currentSpace;
			tags = s.tags;
			edgeTypes = s.edgeTypes;
		});
		const unsub2 = dataBrowserStore.subscribe((s) => {
			activeTab = s.activeTab;
			selectedTag = s.selectedTag;
			selectedEdgeType = s.selectedEdgeType;
			vertices = s.vertices;
			edges = s.edges;
			vertexTotal = s.vertexTotal;
			edgeTotal = s.edgeTotal;
			vertexPage = s.vertexPage;
			edgePage = s.edgePage;
			vertexPageSize = s.vertexPageSize;
			edgePageSize = s.edgePageSize;
			loading = s.loading;
			error = s.error;
			statistics = s.statistics;
			filterPanelVisible = s.filterPanelVisible;
			detailModalVisible = s.detailModalVisible;
			detailData = s.detailData;
			detailType = s.detailType;
		});
		if (currentSpace) schemaStore.fetchTags(currentSpace);
		pageInitialized = true;
		return () => {
			unsub1();
			unsub2();
		};
	});

	async function loadVertices() {
		if (!currentSpace || !selectedTag) return;
		dataBrowserStore.setLoading(true);
		dataBrowserStore.setError(null);
		try {
			const activeFilters = get(dataBrowserStore).filters;
			const response = await dataBrowserService.getVertices(
				currentSpace,
				selectedTag,
				vertexPage,
				vertexPageSize,
				{ field: 'id', order: 'asc' },
				activeFilters,
			);
			dataBrowserStore.setVertices(response.data, response.total);
			if (response.data.length > 0)
				vertexProperties = Object.keys(response.data[0].properties);
		} catch (err) {
			dataBrowserStore.setError(
				err instanceof Error
					? err.message
					: t('notification.loadVerticesFailed'),
			);
		} finally {
			dataBrowserStore.setLoading(false);
		}
	}

	async function loadEdges() {
		if (!currentSpace || !selectedEdgeType) return;
		dataBrowserStore.setLoading(true);
		dataBrowserStore.setError(null);
		try {
			const activeFilters = get(dataBrowserStore).filters;
			const response = await dataBrowserService.getEdges(
				currentSpace,
				selectedEdgeType,
				edgePage,
				edgePageSize,
				{ field: 'id', order: 'asc' },
				activeFilters,
			);
			dataBrowserStore.setEdges(response.data, response.total);
			if (response.data.length > 0)
				edgeProperties = Object.keys(response.data[0].properties);
		} catch (err) {
			dataBrowserStore.setError(
				err instanceof Error ? err.message : t('notification.loadEdgesFailed'),
			);
		} finally {
			dataBrowserStore.setLoading(false);
		}
	}

	function applyFilters() {
		if (activeTab === 'vertices') {
			dataBrowserStore.setVertexPage(1);
			loadVertices();
		} else {
			dataBrowserStore.setEdgePage(1);
			loadEdges();
		}
	}

	async function loadStatistics() {
		if (!currentSpace) return;
		try {
			const stats = await dataBrowserService.getStatistics(currentSpace);
			dataBrowserStore.setStatistics(stats);
		} catch (err) {
			console.error('Failed to load statistics:', err);
		}
	}

	function handleTagChange(e: Event) {
		const val = (e.target as HTMLSelectElement).value || null;
		dataBrowserStore.setSelectedTag(val);
		if (val && currentSpace) loadVertices();
	}

	function handleEdgeTypeChange(e: Event) {
		const val = (e.target as HTMLSelectElement).value || null;
		dataBrowserStore.setSelectedEdgeType(val);
		if (val && currentSpace) loadEdges();
	}

	function handleTabChange(tab: DataBrowserTab) {
		dataBrowserStore.setActiveTab(tab);
		if (tab === 'vertices' && currentSpace) {
			schemaStore.fetchTags(currentSpace);
			if (selectedTag) loadVertices();
		}
		if (tab === 'edges' && currentSpace) {
			schemaStore.fetchEdgeTypes(currentSpace);
			if (selectedEdgeType) loadEdges();
		}
	}

	function showDetail(data: VertexData | EdgeData, type: 'vertex' | 'edge') {
		dataBrowserStore.showDetail(data, type);
	}

	async function copyText(value: string) {
		try {
			await navigator.clipboard.writeText(value);
		} catch {
			/* clipboard unavailable */
		}
	}

	// --- Edit preview with optimistic conflict protection ---

	type EditTarget =
		{ kind: 'vertex'; data: VertexData } | { kind: 'edge'; data: EdgeData };

	let editOpen = $state(false);
	let editTarget = $state<EditTarget | null>(null);
	let editOriginalProps = $state<Record<string, unknown>>({});
	let editedProps = $state<Record<string, unknown>>({});
	let editBusy = $state(false);
	let editError = $state<string | null>(null);
	let editDone = $state(false);
	let conflictState = $state<'none' | 'detected'>('none');

	function editStatement(props: Record<string, unknown>): string {
		if (!editTarget) return '';
		if (editTarget.kind === 'vertex') {
			return buildVertexUpdate(String(editTarget.data.id), props);
		}
		const e = editTarget.data as EdgeData;
		return buildEdgeUpdate(
			e.type ?? 'unknown',
			String(e.src),
			String(e.dst),
			e.rank ?? 0,
			props,
		);
	}

	function openEdit(data: VertexData | EdgeData, kind: 'vertex' | 'edge') {
		const props = { ...(data.properties ?? {}) };
		editTarget =
			kind === 'vertex'
				? { kind, data: data as VertexData }
				: { kind, data: data as EdgeData };
		editOriginalProps = { ...props };
		editedProps = props;
		editOpen = true;
		editError = null;
		editDone = false;
		conflictState = 'none';
	}

	function closeEdit() {
		editOpen = false;
		editTarget = null;
	}

	async function refreshTarget(): Promise<Record<string, unknown> | null> {
		// Re-read the row being edited so a concurrent change can be detected
		// before the update statement is submitted.
		if (!currentSpace || !editTarget) return null;
		try {
			if (editTarget.kind === 'vertex') {
				const tag = (editTarget.data as VertexData).tag ?? '';
				const resp = await dataBrowserService.getVertices(
					currentSpace,
					tag,
					1,
					1,
					{ field: 'id', order: 'asc' },
					{
						conditions: [
							{
								property: 'id',
								operator: 'eq',
								value: String(editTarget.data.id),
							},
						],
						logic: 'AND',
					},
				);
				return resp.data[0]?.properties ?? null;
			}
			const e = editTarget.data as EdgeData;
			const resp = await dataBrowserService.getEdges(
				currentSpace,
				e.type ?? '',
				1,
				1,
				{ field: 'id', order: 'asc' },
				{
					conditions: [{ property: 'id', operator: 'eq', value: String(e.id) }],
					logic: 'AND',
				},
			);
			return resp.data[0]?.properties ?? null;
		} catch {
			// Refresh failure must not block the edit; the server still rejects
			// invalid statements, so proceed without conflict detection.
			return null;
		}
	}

	async function confirmEdit(rebase = false) {
		if (!editTarget || editBusy) return;
		editBusy = true;
		editError = null;
		try {
			if (!editDone) {
				const latest = await refreshTarget();
				if (
					latest !== null &&
					JSON.stringify(latest) !== JSON.stringify(editOriginalProps) &&
					!rebase
				) {
					// Someone else changed the row; surface both sides and stop.
					conflictState = 'detected';
					editOriginalProps = { ...latest };
					return;
				}
				conflictState = 'none';
				const outcome = await queryService.execute({
					query: editStatement(editedProps),
				});
				if (!outcome.success) {
					editError = outcome.error?.message ?? t('errors.executeQuery');
					return;
				}
				editDone = true;
				editOriginalProps = { ...editedProps };
				if (editTarget.kind === 'vertex') loadVertices();
				else loadEdges();
			} else {
				// Rebase mode: the user keeps editing from the latest values and
				// submits a fresh statement on the next confirm.
				editDone = false;
				conflictState = 'none';
			}
		} finally {
			editBusy = false;
		}
	}

	// --- Delete protection ---

	let deleteConfirm = $state<{
		kind: 'vertex' | 'edge';
		statement: string;
		edgeCount: number | null;
	} | null>(null);
	let deleteBusy = $state(false);
	let deleteError = $state<string | null>(null);

	async function requestDelete(
		data: VertexData | EdgeData,
		kind: 'vertex' | 'edge',
	) {
		const statement =
			kind === 'vertex'
				? buildVertexDelete(String(data.id))
				: buildEdgeDelete(
						(data as EdgeData).type ?? 'unknown',
						String((data as EdgeData).src),
						String((data as EdgeData).dst),
						(data as EdgeData).rank ?? 0,
					);
		// Vertex deletion cascades to linked edges, so count them first.
		let edgeCount: number | null = null;
		if (kind === 'vertex' && currentSpace) {
			try {
				const v = data as VertexData;
				for (const et of edgeTypes) {
					const resp = await dataBrowserService.getEdges(
						currentSpace,
						et.name,
						1,
						1,
						{ field: 'id', order: 'asc' },
						{
							conditions: [
								{ property: 'src', operator: 'eq', value: String(v.id) },
							],
							logic: 'OR',
						},
					);
					edgeCount = (edgeCount ?? 0) + resp.total;
				}
			} catch {
				edgeCount = null;
			}
		}
		deleteConfirm = { kind, statement, edgeCount };
		deleteError = null;
	}

	async function confirmDelete() {
		if (!deleteConfirm || deleteBusy) return;
		const kind = deleteConfirm.kind;
		deleteBusy = true;
		deleteError = null;
		try {
			const outcome = await queryService.execute({
				query: deleteConfirm.statement,
			});
			if (!outcome.success) {
				deleteError = outcome.error?.message ?? t('errors.executeQuery');
				return;
			}
			deleteConfirm = null;
			if (kind === 'vertex') loadVertices();
			else if (activeTab === 'vertices') loadVertices();
			else loadEdges();
		} finally {
			deleteBusy = false;
		}
	}

	// --- Batch delete ---

	let selectedIds = new SvelteSet<string>();
	let batchBusy = $state(false);
	let batchMessage = $state<string | null>(null);
	let batchError = $state<string | null>(null);

	function toggleSelect(id: string) {
		if (selectedIds.has(id)) selectedIds.delete(id);
		else selectedIds.add(id);
	}

	function clearSelection() {
		selectedIds.clear();
		batchMessage = null;
		batchError = null;
	}

	async function batchDelete() {
		const count = selectedIds.size;
		if (count === 0 || batchBusy) return;
		if (!confirm(t('dataBrowser.batch.confirmDelete', { count }))) return;
		batchBusy = true;
		batchError = null;
		batchMessage = null;
		try {
			const statements: string[] = [];
			if (activeTab === 'vertices') {
				for (const v of vertices) {
					if (selectedIds.has(String(v.id)))
						statements.push(buildVertexDelete(String(v.id)));
				}
			} else {
				for (const e of edges) {
					if (selectedIds.has(String(e.id)))
						statements.push(
							buildEdgeDelete(
								e.type ?? 'unknown',
								String(e.src),
								String(e.dst),
								e.rank ?? 0,
							),
						);
				}
			}
			const script = statements.join(';\n');
			const outcome = await queryService.executeBatch(script);
			const failed = outcome.results.filter((r) => !r.success);
			const ok = outcome.results.length - failed.length;
			batchMessage = t('dataBrowser.batch.done', { ok, failed: failed.length });
			if (failed.length > 0) {
				const first = outcome.results.findIndex((r) => !r.success);
				batchError = t('dataBrowser.batch.failedAt', {
					index: first + 1,
					message: failed[0].error?.message ?? '',
				});
			}
			selectedIds.clear();
			if (activeTab === 'vertices') loadVertices();
			else loadEdges();
		} catch (err) {
			batchError =
				err instanceof Error ? err.message : t('errors.executeBatch');
		} finally {
			batchBusy = false;
		}
	}

	function viewInGraph(data: VertexData | EdgeData, type: 'vertex' | 'edge') {
		if (type === 'vertex') {
			const v = data as VertexData;
			graphStore.mergeGraphData({
				nodes: [
					{
						id: String(v.id),
						tag: v.tag ?? 'unknown',
						properties: v.properties ?? {},
					},
				],
				edges: [],
			});
		} else {
			const e = data as EdgeData;
			const src = String(e.src);
			const dst = String(e.dst);
			graphStore.mergeGraphData({
				nodes: [
					{ id: src, tag: 'unknown', properties: {} },
					{ id: dst, tag: 'unknown', properties: {} },
				],
				edges: [
					{
						id: String(e.id),
						type: e.type ?? 'unknown',
						source: src,
						target: dst,
						rank: e.rank ?? 0,
						properties: e.properties ?? {},
					},
				],
			});
		}
		goto('/graph');
	}
</script>

{#if !pageInitialized}
	<PageSkeleton />
{:else if currentSpace}
	<div class="ui-page">
		<PageHeader title={t('sidebar.dataBrowser')}>
			<button class="ui-btn ui-btn-ghost ui-btn-sm" onclick={loadStatistics}>
				<RefreshCw size={16} /> {t('common.refresh')}
			</button>
			{#if activeTab === 'vertices' || activeTab === 'edges'}
				<button
					class="ui-chip {filterPanelVisible ? 'ui-chip-active' : ''}"
					onclick={() => dataBrowserStore.toggleFilterPanel()}
				>
					<Search size={16} /> {t('dataBrowser.filter')}
				</button>
			{/if}
		</PageHeader>

		{#if filterPanelVisible && (activeTab === 'vertices' || activeTab === 'edges')}
			<FilterPanel {activeTab} onApply={applyFilters} />
		{/if}

		{#if error}
			<div class="ui-alert ui-alert-error text-sm">
				{error}
			</div>
		{/if}

		{#if selectedIds.size > 0 && (activeTab === 'vertices' || activeTab === 'edges')}
			<div class="ui-alert ui-alert-info flex items-center gap-3 text-sm">
				<span class="text-info">
					{t('dataBrowser.batch.selected', { count: selectedIds.size })}
				</span>
				<button
					class="ui-btn ui-btn-danger ui-btn-sm"
					disabled={batchBusy}
					onclick={batchDelete}
				>
					{t('dataBrowser.batch.deleteSelected')}
				</button>
				<button class="ui-btn ui-btn-ghost ui-btn-sm" onclick={clearSelection}>
					{t('common.clear')}
				</button>
			</div>
		{/if}
		{#if batchMessage}
			<div class="ui-alert ui-alert-ok text-sm">
				{batchMessage}
				{#if batchError}
					<div class="mt-1 text-error text-xs">
						{batchError}
					</div>
				{/if}
			</div>
		{/if}

		<div class="flex-1 ui-card ui-card-flush shadow-sm overflow-hidden flex">
			<div class="flex-1 flex flex-col overflow-hidden">
				<div class="flex border-b border-line">
					<button
						class="ui-tab {activeTab === 'vertices' ? 'ui-tab-active' : ''}"
						onclick={() => handleTabChange('vertices')}
					>
						<Package size={16} /> {t('dataBrowser.vertices')}
					</button>
					<button
						class="ui-tab {activeTab === 'edges' ? 'ui-tab-active' : ''}"
						onclick={() => handleTabChange('edges')}
					>
						<ArrowLeftRight size={16} /> {t('sidebar.edges')}
					</button>
					<button
						class="ui-tab {activeTab === 'vector' ? 'ui-tab-active' : ''}"
						onclick={() => handleTabChange('vector')}
					>
						🧭 {t('dataBrowser.vectorTab')}
					</button>
					<button
						class="ui-tab {activeTab === 'fulltext' ? 'ui-tab-active' : ''}"
						onclick={() => handleTabChange('fulltext')}
					>
						🔎 {t('dataBrowser.fulltextTab')}
					</button>
				</div>

				<div class="p-5 flex-1 overflow-auto">
					{#if activeTab === 'vector'}
						<VectorSearchPanel {currentSpace} />
					{:else if activeTab === 'fulltext'}
						<FulltextSearchPanel {currentSpace} />
					{:else if activeTab === 'vertices'}
						<div class="mb-4">
							<select
								class="ui-input"
								value={selectedTag || ''}
								onchange={handleTagChange}
							>
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
											{#each vertexProperties as prop (prop)}
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
														onchange={() => toggleSelect(String(v.id))}
													/>
												</td>
												<td class="font-mono">{v.id}</td>
												<td>{v.tag}</td>
												{#each vertexProperties as prop (prop)}
													<td class="max-w-40 truncate"
														>{formatCellValue(v.properties[prop])}</td
													>
												{/each}
												<td>
													<button
														class="ui-link text-xs"
														onclick={() => showDetail(v, 'vertex')}
														>{t('dataBrowser.viewDetail')}</button
													>
													<button
														class="ml-2 text-success hover:text-success text-xs cursor-pointer"
														onclick={() => viewInGraph(v, 'vertex')}
														>{t('graphPreview.openInGraph')}</button
													>
													<button
														class="ml-2 text-muted hover:text-ink text-xs cursor-pointer"
														onclick={() => copyText(String(v.id))}
														>{t('dataBrowser.copyId')}</button
													>
													<button
														class="ml-2 text-warning hover:text-warning text-xs cursor-pointer"
														onclick={() => openEdit(v, 'vertex')}
														>{t('dataBrowser.editUpdate')}</button
													>
													<button
														class="ml-2 ui-link-danger text-xs"
														onclick={() => requestDelete(v, 'vertex')}
														>{t('dataBrowser.editDelete')}</button
													>
												</td>
											</tr>
										{/each}
									</tbody>
								</table>
							</div>
							<div
								class="flex items-center justify-between mt-4 text-sm text-muted"
							>
								<span
									>{t('dataBrowser.total')}: {vertexTotal}
									{t('dataBrowser.items')}</span
								>
								<div class="flex gap-2">
									<button
										class="ui-btn ui-btn-ghost ui-btn-sm"
										disabled={vertexPage <= 1}
										onclick={() => {
											dataBrowserStore.setVertexPage(vertexPage - 1);
											loadVertices();
										}}>{t('common.prev')}</button
									>
									<span class="px-3 py-1 text-muted"
										>{t('common.page')} {vertexPage}</span
									>
									<button
										class="ui-btn ui-btn-ghost ui-btn-sm"
										disabled={vertexPage * vertexPageSize >= vertexTotal}
										onclick={() => {
											dataBrowserStore.setVertexPage(vertexPage + 1);
											loadVertices();
										}}>{t('common.next')}</button
									>
								</div>
							</div>
						{:else if selectedTag}
							<p class="text-muted text-center py-8">
								{t('schema.noTags')}
							</p>
						{:else}
							<p class="text-muted text-center py-8">
								{t('dataBrowser.selectTag')}
								{t('common.loading')}
							</p>
						{/if}
					{:else}
						<div class="mb-4">
							<select
								class="ui-input"
								value={selectedEdgeType || ''}
								onchange={handleEdgeTypeChange}
							>
								<option value="">-- {t('dataBrowser.selectEdgeType')} --</option
								>
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
											{#each edgeProperties as prop (prop)}
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
														onchange={() => toggleSelect(String(e.id))}
													/>
												</td>
												<td class="font-mono">{e.id}</td>
												<td>{e.type}</td>
												<td class="font-mono">{e.src}</td>
												<td class="font-mono">{e.dst}</td>
												<td>{e.rank}</td>
												{#each edgeProperties as prop (prop)}
													<td class="max-w-40 truncate"
														>{formatCellValue(e.properties[prop])}</td
													>
												{/each}
												<td>
													<button
														class="ui-link text-xs"
														onclick={() => showDetail(e, 'edge')}
														>{t('dataBrowser.viewDetail')}</button
													>
													<button
														class="ml-2 text-success hover:text-success text-xs cursor-pointer"
														onclick={() => viewInGraph(e, 'edge')}
														>{t('graphPreview.openInGraph')}</button
													>
													<button
														class="ml-2 text-muted hover:text-ink text-xs cursor-pointer"
														onclick={() => copyText(String(e.src))}
														>{t('dataBrowser.copyId')}</button
													>
													<button
														class="ml-2 text-warning hover:text-warning text-xs cursor-pointer"
														onclick={() => openEdit(e, 'edge')}
														>{t('dataBrowser.editUpdate')}</button
													>
													<button
														class="ml-2 ui-link-danger text-xs"
														onclick={() => requestDelete(e, 'edge')}
														>{t('dataBrowser.editDelete')}</button
													>
												</td>
											</tr>
										{/each}
									</tbody>
								</table>
							</div>
							<div
								class="flex items-center justify-between mt-4 text-sm text-muted"
							>
								<span
									>{t('dataBrowser.total')}: {edgeTotal}
									{t('dataBrowser.items')}</span
								>
								<div class="flex gap-2">
									<button
										class="ui-btn ui-btn-ghost ui-btn-sm"
										disabled={edgePage <= 1}
										onclick={() => {
											dataBrowserStore.setEdgePage(edgePage - 1);
											loadEdges();
										}}>{t('common.prev')}</button
									>
									<span class="px-3 py-1 text-muted"
										>{t('common.page')} {edgePage}</span
									>
									<button
										class="ui-btn ui-btn-ghost ui-btn-sm"
										disabled={edgePage * edgePageSize >= edgeTotal}
										onclick={() => {
											dataBrowserStore.setEdgePage(edgePage + 1);
											loadEdges();
										}}>{t('common.next')}</button
									>
								</div>
							</div>
						{:else if selectedEdgeType}
							<p class="text-muted text-center py-8">
								{t('schema.noEdges')}
							</p>
						{:else}
							<p class="text-muted text-center py-8">
								{t('dataBrowser.selectEdgeType')}
								{t('common.loading')}
							</p>
						{/if}
					{/if}
				</div>
			</div>

			<!-- Statistics Panel -->
			<div
				class="w-64 border-l border-line p-5 bg-secondary overflow-y-auto"
			>
				<h2
					class="text-base font-semibold text-ink mb-3"
				>
					{t('dataBrowser.statistics')}
				</h2>
				{#if statistics}
					<div class="space-y-2 text-sm">
						<div class="flex justify-between">
							<span class="text-muted"
								>{t('dataBrowser.vertices')}:</span
							><span class="font-medium text-ink"
								>{statistics.totalVertices ?? '-'}</span
							>
						</div>
						<div class="flex justify-between">
							<span class="text-muted"
								>{t('dataBrowser.edges')}:</span
							><span class="font-medium text-ink"
								>{statistics.totalEdges ?? '-'}</span
							>
						</div>
						<div class="flex justify-between">
							<span class="text-muted"
								>{t('dataBrowser.tags')}:</span
							><span class="font-medium text-ink"
								>{statistics.tagCount ?? '-'}</span
							>
						</div>
						<div class="flex justify-between">
							<span class="text-muted"
								>{t('dataBrowser.edgeTypes')}:</span
							><span class="font-medium text-ink"
								>{statistics.edgeTypeCount ?? '-'}</span
							>
						</div>
					</div>
				{:else}
					<p class="text-muted text-xs">
						{t('common.refresh')}
						{t('common.loading')}
					</p>
				{/if}
			</div>
		</div>
	</div>
{:else}
	<div class="ui-card shadow-sm p-8 text-center">
		<p class="text-muted">
			{t('common.select')}
			{t('sidebar.spaces')}
			{t('common.loading')}
		</p>
	</div>
{/if}

<!-- Detail Modal -->
{#if detailModalVisible && detailData}
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
						<h3
							class="text-sm font-medium text-ink mb-2"
						>
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
{/if}

<!-- Edit Preview Modal -->
<EditPreviewModal
	open={editOpen && conflictState === 'none'}
	title={editTarget?.kind === 'vertex'
		? t('dataBrowser.edit.titleVertex')
		: t('dataBrowser.edit.titleEdge')}
	original={editOriginalProps}
	edited={editedProps}
	statement={editStatement(editedProps)}
	busy={editBusy}
	errorMessage={editError}
	onConfirm={() => confirmEdit(false)}
	onClose={closeEdit}
	onEditedChange={(values) => (editedProps = values)}
/>

<!-- Conflict Modal -->
{#if editOpen && conflictState === 'detected'}
	<div class="fixed inset-0 z-50 flex items-center justify-center">
		<div
			role="presentation"
			class="absolute inset-0 bg-mask"
			onclick={() => (conflictState = 'none')}
		></div>
		<div
			class="relative ui-card shadow-lg p-6 w-[28rem] max-h-[85vh] overflow-y-auto"
		>
			<h2 class="text-base font-semibold text-ink mb-2">
				{t('dataBrowser.edit.changed')}
			</h2>
			<p class="text-sm text-muted mb-3">
				{t('dataBrowser.edit.conflict')}
			</p>
			<div class="space-y-1">
				{#each Object.keys(editOriginalProps) as key (key)}
					<div class="text-xs flex gap-2 font-mono">
						<span class="text-muted w-24 truncate"
							>{key}</span
						>
						<span class="text-ink flex-1 truncate"
							>{String(editOriginalProps[key] ?? '')}</span
						>
						<span class="text-warning flex-1 truncate"
							>{String(editedProps[key] ?? '')}</span
						>
					</div>
				{/each}
			</div>
			<div class="mt-4 flex justify-end gap-2">
				<button class="ui-btn ui-btn-ghost" onclick={closeEdit}
					>{t('dataBrowser.edit.conflictDiscard')}</button
				>
				<button
					class="ui-btn ui-btn-primary"
					disabled={editBusy}
					onclick={() => confirmEdit(true)}
				>
					{t('dataBrowser.edit.conflictRebase')}
				</button>
			</div>
		</div>
	</div>
{/if}

<!-- Delete Confirm Modal -->
{#if deleteConfirm}
	<div class="fixed inset-0 z-50 flex items-center justify-center">
		<div
			role="presentation"
			class="absolute inset-0 bg-mask"
			onclick={() => (deleteConfirm = null)}
		></div>
		<div
			class="relative ui-card shadow-lg p-6 w-[26rem] max-h-[85vh] overflow-y-auto"
		>
			<h2 class="text-base font-semibold text-ink mb-2">
				{t('common.delete')}
			</h2>
			<p class="text-sm text-muted">
				{t('dataBrowser.delete.confirm', {
					kind:
						deleteConfirm.kind === 'vertex'
							? t('dataBrowser.vertices')
							: t('sidebar.edges'),
				})}
			</p>
			{#if deleteConfirm.kind === 'vertex' && deleteConfirm.edgeCount !== null && deleteConfirm.edgeCount > 0}
				<p class="text-sm text-warning mt-2">
					{t('dataBrowser.delete.vertexWithEdges', {
						count: deleteConfirm.edgeCount,
					})}
				</p>
			{/if}
			<pre
				class="ui-pre ui-pre-sm whitespace-pre-wrap mt-3">{deleteConfirm.statement}</pre>
			{#if deleteError}
				<div class="ui-alert ui-alert-error mt-3">
					{deleteError}
				</div>
			{/if}
			<div class="mt-4 flex justify-end gap-2">
				<button
					class="ui-btn ui-btn-ghost"
					onclick={() => (deleteConfirm = null)}>{t('common.cancel')}</button
				>
				<button
					class="ui-btn ui-btn-danger"
					disabled={deleteBusy}
					onclick={confirmDelete}
				>
					{deleteBusy ? t('console.executing') : t('common.delete')}
				</button>
			</div>
		</div>
	</div>
{/if}

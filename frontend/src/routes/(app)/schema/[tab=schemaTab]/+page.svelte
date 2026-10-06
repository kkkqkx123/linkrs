<script lang="ts">
	import { onMount } from 'svelte';
	import { t, type MessageKey } from '$i18n';
	import { functionsService } from '$services/functions';
	import { schemaVersionsService } from '$services/schemaVersions';
	import { schemaStore } from '$stores/schema';
	import { theme } from '$stores/theme';
	import { DATA_TYPE_LABELS } from '$config/constants';
	import PageSkeleton from '$components/common/PageSkeleton.svelte';
	import SchemaErGraph from '$components/business/SchemaErGraph.svelte';
	import SchemaAlterModal from '$components/business/SchemaAlterModal.svelte';
	import type { Space, Tag, EdgeType } from '$types/schema';
	import type { components } from '$lib/api/schema';

	type SchemaTab =
		| 'spaces'
		| 'tags'
		| 'edges'
		| 'indexes'
		| 'visualization'
		| 'functions'
		| 'versions';

	const TABS: { id: SchemaTab; label: MessageKey }[] = [
		{ id: 'spaces', label: 'sidebar.spaces' },
		{ id: 'tags', label: 'sidebar.tags' },
		{ id: 'edges', label: 'sidebar.edges' },
		{ id: 'indexes', label: 'sidebar.indexes' },
		{ id: 'visualization', label: 'sidebar.visualization' },
		{ id: 'functions', label: 'sidebar.functions' },
		{ id: 'versions', label: 'sidebar.versions' },
	];

	type IndexInfo = components['schemas']['IndexInfo'] & {
		entity_type?: string;
		entity_name?: string;
	};

	let { data } = $props();
	let activeTab = $derived(data.tab);
	let pageInitialized = $state(false);
	let isDark = $state(false);

	// Spaces
	let spaces = $state<Space[]>([]);
	let isLoadingSpaces = $state(false);
	let currentSpace = $state<string | null>(null);
	let showCreateSpace = $state(false);
	let newSpaceName = $state('');
	let newSpaceVidType = $state('INT64');
	let newSpacePartitionNum = $state(7);
	let newSpaceReplicaFactor = $state(1);

	// Tags
	let tags = $state<Tag[]>([]);
	let isLoadingTags = $state(false);
	let showCreateTag = $state(false);
	let newTagName = $state('');
	let newTagProps = $state<
		Array<{ name: string; data_type: string; nullable: boolean }>
	>([]);

	// Edges
	let edgeTypes = $state<EdgeType[]>([]);
	let isLoadingEdgeTypes = $state(false);
	let showCreateEdge = $state(false);
	let newEdgeName = $state('');
	let newEdgeProps = $state<
		Array<{ name: string; data_type: string; nullable: boolean }>
	>([]);

	// Indexes
	let indexes = $state<IndexInfo[]>([]);
	let isLoadingIndexes = $state(false);
	let showCreateIndex = $state(false);
	let newIndexName = $state('');
	let newIndexType = $state('INDEX');
	let newIndexEntityType = $state('TAG');
	let newIndexEntityName = $state('');
	let newIndexFields = $state('');

	// Alter dialogs
	let alterTagName = $state<string | null>(null);
	let alterEdgeName = $state<string | null>(null);
	let alterBusy = $state(false);

	// Functions
	let functions = $state<string[]>([]);
	let isLoadingFunctions = $state(false);
	let functionsError = $state<string | null>(null);
	let functionsResult = $state('');
	let newFunctionName = $state('');
	let newFunctionImpl = $state('');

	// Versions
	let versionSpace = $state('');
	let versionLabel = $state('');
	let versionIsEdge = $state(false);
	let versionFrom = $state(1);
	let versionTo = $state(2);
	let isLoadingVersions = $state(false);
	let versionsError = $state<string | null>(null);
	let versionsResult = $state('');

	function stringifyPayload(payload: unknown): string {
		try {
			return JSON.stringify(payload, null, 2);
		} catch {
			return String(payload ?? '');
		}
	}

	function functionNames(payload: unknown): string[] {
		if (Array.isArray(payload)) {
			return payload
				.map((item) =>
					typeof item === 'string'
						? item
						: (item as Record<string, unknown>)?.name,
				)
				.filter((name): name is string => typeof name === 'string');
		}
		if (payload && typeof payload === 'object') {
			const record = payload as Record<string, unknown>;
			const list = record.functions;
			if (Array.isArray(list)) return functionNames(list);
			if (typeof record.name === 'string') return [record.name];
		}
		return [];
	}

	async function loadFunctions() {
		isLoadingFunctions = true;
		functionsError = null;
		try {
			const payload = await functionsService.list();
			functions = functionNames(payload);
			functionsResult = stringifyPayload(payload);
		} catch (err) {
			functionsError =
				err instanceof Error ? err.message : t('notification.requestFailed');
		} finally {
			isLoadingFunctions = false;
		}
	}

	async function showFunctionInfo(name: string) {
		isLoadingFunctions = true;
		functionsError = null;
		try {
			functionsResult = stringifyPayload(await functionsService.info(name));
		} catch (err) {
			functionsError =
				err instanceof Error ? err.message : t('notification.requestFailed');
		} finally {
			isLoadingFunctions = false;
		}
	}

	async function registerFunction() {
		isLoadingFunctions = true;
		functionsError = null;
		try {
			functionsResult = stringifyPayload(
				await functionsService.register({
					name: newFunctionName,
					implementation: newFunctionImpl,
				}),
			);
			newFunctionName = '';
			newFunctionImpl = '';
			await loadFunctions();
		} catch (err) {
			functionsError =
				err instanceof Error ? err.message : t('notification.requestFailed');
		} finally {
			isLoadingFunctions = false;
		}
	}

	async function unregisterFunction(name: string) {
		isLoadingFunctions = true;
		functionsError = null;
		try {
			functionsResult = stringifyPayload(
				await functionsService.unregister(name),
			);
			await loadFunctions();
		} catch (err) {
			functionsError =
				err instanceof Error ? err.message : t('notification.requestFailed');
		} finally {
			isLoadingFunctions = false;
		}
	}

	async function runVersionQuery(fn: () => Promise<unknown>) {
		isLoadingVersions = true;
		versionsError = null;
		try {
			versionsResult = stringifyPayload(await fn());
		} catch (err) {
			versionsError =
				err instanceof Error ? err.message : t('notification.requestFailed');
		} finally {
			isLoadingVersions = false;
		}
	}

	const dataTypes = Object.values(DATA_TYPE_LABELS).filter(Boolean);

	onMount(() => {
		const unsub = schemaStore.subscribe((s) => {
			spaces = s.spaces;
			isLoadingSpaces = s.isLoadingSpaces;
			currentSpace = s.currentSpace;
			tags = s.tags;
			isLoadingTags = s.isLoadingTags;
			edgeTypes = s.edgeTypes;
			isLoadingEdgeTypes = s.isLoadingEdgeTypes;
			indexes = s.indexes;
			isLoadingIndexes = s.isLoadingIndexes;
		});
		const unsubTheme = theme.subscribe((v) => {
			isDark = v === 'dark';
		});
		schemaStore.fetchSpaces().finally(() => {
			pageInitialized = true;
		});
		return () => {
			unsub();
			unsubTheme();
		};
	});

	function selectSpace(name: string) {
		schemaStore.setCurrentSpace(name);
		if (activeTab === 'tags') schemaStore.fetchTags(name);
		if (activeTab === 'edges') schemaStore.fetchEdgeTypes(name);
		if (activeTab === 'indexes') schemaStore.fetchIndexes(name);
		if (activeTab === 'visualization') {
			schemaStore.fetchTags(name);
			schemaStore.fetchEdgeTypes(name);
		}
	}

	$effect(() => {
		const tab = activeTab;
		if (tab === 'spaces') return;
		if (!currentSpace) return;
		if (tab === 'tags') schemaStore.fetchTags(currentSpace);
		if (tab === 'edges') schemaStore.fetchEdgeTypes(currentSpace);
		if (tab === 'indexes') schemaStore.fetchIndexes(currentSpace);
		if (tab === 'visualization') {
			schemaStore.fetchTags(currentSpace);
			schemaStore.fetchEdgeTypes(currentSpace);
		}
	});

	async function createSpace() {
		if (!newSpaceName.trim()) return;
		await schemaStore.createSpace({
			name: newSpaceName,
			vidType: newSpaceVidType as 'INT64' | 'FIXED_STRING(32)',
			partitionNum: newSpacePartitionNum,
			replicaFactor: newSpaceReplicaFactor,
		});
		showCreateSpace = false;
		newSpaceName = '';
	}

	async function deleteSpace(name: string) {
		if (confirm(t('common.confirmDelete', { name }))) {
			await schemaStore.deleteSpace(name);
		}
	}

	async function createTag() {
		if (!newTagName.trim() || !currentSpace) return;
		await schemaStore.createTag(currentSpace, {
			name: newTagName,
			properties: newTagProps.filter((p) => p.name.trim()),
		});
		showCreateTag = false;
		newTagName = '';
		newTagProps = [];
	}

	async function deleteTag(tagName: string) {
		if (
			currentSpace &&
			confirm(t('common.confirmDeleteItem', { name: tagName }))
		) {
			await schemaStore.deleteTag(currentSpace, tagName);
		}
	}

	async function createEdge() {
		if (!newEdgeName.trim() || !currentSpace) return;
		await schemaStore.createEdgeType(currentSpace, {
			name: newEdgeName,
			properties: newEdgeProps.filter((p) => p.name.trim()),
		});
		showCreateEdge = false;
		newEdgeName = '';
		newEdgeProps = [];
	}

	async function deleteEdge(edgeName: string) {
		if (
			currentSpace &&
			confirm(t('common.confirmDeleteItem', { name: edgeName }))
		) {
			await schemaStore.deleteEdgeType(currentSpace, edgeName);
		}
	}

	async function createIndex() {
		if (!newIndexName.trim() || !newIndexEntityName.trim() || !currentSpace)
			return;
		await schemaStore.createIndex(currentSpace, {
			name: newIndexName,
			index_type: newIndexType,
			entity_type: newIndexEntityType,
			entity_name: newIndexEntityName,
			fields: newIndexFields
				.split(',')
				.map((f) => f.trim())
				.filter(Boolean),
		});
		showCreateIndex = false;
		newIndexName = '';
		newIndexFields = '';
	}

	async function deleteIndex(indexName: string) {
		if (
			currentSpace &&
			confirm(t('common.confirmDeleteItem', { name: indexName }))
		) {
			await schemaStore.deleteIndex(currentSpace, indexName);
		}
	}

	async function rebuildIndex(indexName: string) {
		if (!currentSpace) return;
		await schemaStore.rebuildIndex(currentSpace, indexName);
	}

	async function submitAlterTag(
		added: Array<{ name: string; data_type: string; nullable: boolean }>,
		dropped: string[],
	) {
		if (!currentSpace || !alterTagName) return;
		if (added.length === 0 && dropped.length === 0) {
			alterTagName = null;
			return;
		}
		alterBusy = true;
		try {
			await schemaStore.updateTag(currentSpace, alterTagName, {
				add_properties: added,
				drop_properties: dropped,
			});
			alterTagName = null;
		} finally {
			alterBusy = false;
		}
	}

	async function submitAlterEdge(
		added: Array<{ name: string; data_type: string; nullable: boolean }>,
		dropped: string[],
	) {
		if (!currentSpace || !alterEdgeName) return;
		if (added.length === 0 && dropped.length === 0) {
			alterEdgeName = null;
			return;
		}
		alterBusy = true;
		try {
			await schemaStore.updateEdgeType(currentSpace, alterEdgeName, {
				add_properties: added,
				drop_properties: dropped,
			});
			alterEdgeName = null;
		} finally {
			alterBusy = false;
		}
	}

	function addProp(
		props: Array<{ name: string; data_type: string; nullable: boolean }>,
	) {
		props.push({ name: '', data_type: 'STRING', nullable: true });
	}

	function removeProp(
		props: Array<{ name: string; data_type: string; nullable: boolean }>,
		index: number,
	) {
		props.splice(index, 1);
	}
</script>

{#if !pageInitialized}
	<PageSkeleton />
{:else}
	<div class="flex flex-col h-full gap-4">
		<div class="bg-white dark:bg-[#1C2333] rounded-lg shadow-sm px-5 py-3">
			<h2 class="text-lg font-semibold text-gray-800 dark:text-gray-100">
				{t('schema.title')}
			</h2>
		</div>

		<!-- Space Selector -->
		<div class="bg-white dark:bg-[#1C2333] rounded-lg shadow-sm px-5 py-3">
			<div class="flex items-center gap-4">
				<span class="text-sm font-medium text-gray-600 dark:text-gray-400"
					>{t('sidebar.spaces')}:</span
				>
				<select
					class="px-3 py-1.5 border border-gray-300 dark:border-gray-600 rounded text-sm bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200 focus:outline-none focus:border-blue-500"
					value={currentSpace || ''}
					onchange={(e) => selectSpace((e.target as HTMLSelectElement).value)}
				>
					<option value="">-- {t('common.select')} --</option>
					{#each spaces as s (s.name)}
						<option value={s.name}>{s.name}</option>
					{/each}
				</select>
				<button
					class="px-3 py-1.5 bg-blue-500 hover:bg-blue-600 text-white text-sm rounded cursor-pointer"
					onclick={() => {
						showCreateSpace = true;
					}}
				>
					+ {t('schema.createSpace')}
				</button>
			</div>
		</div>

		<!-- Tabs -->
		<div
			class="bg-white dark:bg-[#1C2333] rounded-lg shadow-sm flex flex-col flex-1 overflow-hidden"
		>
			<div class="flex border-b border-gray-200 dark:border-gray-700">
				{#each TABS as tab (tab.id)}
					<a
						href={`/schema/${tab.id}`}
						class="px-5 py-3 text-sm font-medium transition-colors {activeTab ===
						tab.id
							? 'text-blue-600 dark:text-blue-400 border-b-2 border-blue-500'
							: 'text-gray-500 dark:text-gray-400 hover:text-gray-700 dark:hover:text-gray-300'}"
						aria-current={activeTab === tab.id ? 'page' : undefined}
					>
						{t(tab.label)}
					</a>
				{/each}
			</div>

			<div class="flex-1 overflow-auto p-4">
				{#if activeTab === 'spaces'}
					{#if isLoadingSpaces}
						<div class="flex items-center justify-center p-8">
							<div
								class="animate-spin w-6 h-6 border-2 border-blue-500 border-t-transparent rounded-full"
							></div>
						</div>
					{:else if spaces.length === 0}
						<p class="text-gray-400 dark:text-gray-500 text-center py-8">
							{t('schema.noSpaces')}
						</p>
					{:else}
						<div class="overflow-x-auto">
							<table class="w-full text-sm border-collapse">
								<thead>
									<tr class="bg-gray-50 dark:bg-gray-800/50">
										<th
											class="px-3 py-2 text-left font-medium text-gray-600 dark:text-gray-400 border-b border-gray-200 dark:border-gray-700"
											>{t('common.name')}</th
										>
										<th
											class="px-3 py-2 text-left font-medium text-gray-600 dark:text-gray-400 border-b border-gray-200 dark:border-gray-700"
											>{t('schema.vidType')}</th
										>
										<th
											class="px-3 py-2 text-left font-medium text-gray-600 dark:text-gray-400 border-b border-gray-200 dark:border-gray-700"
											>{t('common.actions')}</th
										>
									</tr>
								</thead>
								<tbody>
									{#each spaces as space (space.name)}
										<tr
											class="hover:bg-gray-50 dark:hover:bg-gray-800/30 {currentSpace ===
											space.name
												? 'bg-blue-50 dark:bg-blue-900/20'
												: ''}"
										>
											<td
												class="px-3 py-2 border-b border-gray-100 dark:border-gray-700/50 font-medium text-gray-800 dark:text-gray-200"
												>{space.name}</td
											>
											<td
												class="px-3 py-2 border-b border-gray-100 dark:border-gray-700/50 text-gray-700 dark:text-gray-300"
												>{space.vid_type}</td
											>
											<td
												class="px-3 py-2 border-b border-gray-100 dark:border-gray-700/50"
											>
												<button
													class="text-red-500 hover:text-red-700 text-xs cursor-pointer"
													onclick={() => deleteSpace(space.name)}
													>{t('common.delete')}</button
												>
											</td>
										</tr>
									{/each}
								</tbody>
							</table>
						</div>
					{/if}
				{:else if activeTab === 'tags'}
					<div class="flex justify-between items-center mb-4">
						<span class="text-sm text-gray-500 dark:text-gray-400"
							>{tags.length} {t('sidebar.tags')}</span
						>
						<button
							class="px-3 py-1.5 bg-blue-500 hover:bg-blue-600 text-white text-sm rounded cursor-pointer"
							onclick={() => {
								showCreateTag = true;
							}}>+ {t('schema.createTag')}</button
						>
					</div>
					{#if isLoadingTags}
						<div class="flex items-center justify-center p-8">
							<div
								class="animate-spin w-6 h-6 border-2 border-blue-500 border-t-transparent rounded-full"
							></div>
						</div>
					{:else if tags.length === 0}
						<p class="text-gray-400 dark:text-gray-500 text-center py-8">
							{t('schema.noTags')}
						</p>
					{:else}
						<div class="overflow-x-auto">
							<table class="w-full text-sm border-collapse">
								<thead>
									<tr class="bg-gray-50 dark:bg-gray-800/50">
										<th
											class="px-3 py-2 text-left font-medium text-gray-600 dark:text-gray-400 border-b border-gray-200 dark:border-gray-700"
											>{t('common.name')}</th
										>
										<th
											class="px-3 py-2 text-left font-medium text-gray-600 dark:text-gray-400 border-b border-gray-200 dark:border-gray-700"
											>{t('common.properties')}</th
										>
										<th
											class="px-3 py-2 text-left font-medium text-gray-600 dark:text-gray-400 border-b border-gray-200 dark:border-gray-700"
											>{t('common.actions')}</th
										>
									</tr>
								</thead>
								<tbody>
									{#each tags as tag (tag.name)}
										<tr class="hover:bg-gray-50 dark:hover:bg-gray-800/30">
											<td
												class="px-3 py-2 border-b border-gray-100 dark:border-gray-700/50 font-medium text-gray-800 dark:text-gray-200"
												>{tag.name}</td
											>
											<td
												class="px-3 py-2 border-b border-gray-100 dark:border-gray-700/50"
											>
												{#if tag.properties?.length}
													<span class="text-xs text-gray-500 dark:text-gray-400"
														>{tag.properties
															.map((p) => p.name)
															.join(', ')}</span
													>
												{:else}
													<span class="text-xs text-gray-400 dark:text-gray-500"
														>{t('common.noProperties')}</span
													>
												{/if}
											</td>
											<td
												class="px-3 py-2 border-b border-gray-100 dark:border-gray-700/50"
											>
												<button
													class="text-blue-500 hover:text-blue-700 text-xs cursor-pointer"
													onclick={() => (alterTagName = tag.name)}
													>{t('schema.alter')}</button
												>
												<button
													class="ml-2 text-red-500 hover:text-red-700 text-xs cursor-pointer"
													onclick={() => deleteTag(tag.name)}
													>{t('common.delete')}</button
												>
											</td>
										</tr>
									{/each}
								</tbody>
							</table>
						</div>
					{/if}
				{:else if activeTab === 'edges'}
					<div class="flex justify-between items-center mb-4">
						<span class="text-sm text-gray-500 dark:text-gray-400"
							>{edgeTypes.length} {t('sidebar.edges')}</span
						>
						<button
							class="px-3 py-1.5 bg-blue-500 hover:bg-blue-600 text-white text-sm rounded cursor-pointer"
							onclick={() => {
								showCreateEdge = true;
							}}>+ {t('schema.createEdge')}</button
						>
					</div>
					{#if isLoadingEdgeTypes}
						<div class="flex items-center justify-center p-8">
							<div
								class="animate-spin w-6 h-6 border-2 border-blue-500 border-t-transparent rounded-full"
							></div>
						</div>
					{:else if edgeTypes.length === 0}
						<p class="text-gray-400 dark:text-gray-500 text-center py-8">
							{t('schema.noEdges')}
						</p>
					{:else}
						<div class="overflow-x-auto">
							<table class="w-full text-sm border-collapse">
								<thead>
									<tr class="bg-gray-50 dark:bg-gray-800/50">
										<th
											class="px-3 py-2 text-left font-medium text-gray-600 dark:text-gray-400 border-b border-gray-200 dark:border-gray-700"
											>{t('common.name')}</th
										>
										<th
											class="px-3 py-2 text-left font-medium text-gray-600 dark:text-gray-400 border-b border-gray-200 dark:border-gray-700"
											>{t('common.properties')}</th
										>
										<th
											class="px-3 py-2 text-left font-medium text-gray-600 dark:text-gray-400 border-b border-gray-200 dark:border-gray-700"
											>{t('common.actions')}</th
										>
									</tr>
								</thead>
								<tbody>
									{#each edgeTypes as edge (edge.name)}
										<tr class="hover:bg-gray-50 dark:hover:bg-gray-800/30">
											<td
												class="px-3 py-2 border-b border-gray-100 dark:border-gray-700/50 font-medium text-gray-800 dark:text-gray-200"
												>{edge.name}</td
											>
											<td
												class="px-3 py-2 border-b border-gray-100 dark:border-gray-700/50"
											>
												{#if edge.properties?.length}
													<span class="text-xs text-gray-500 dark:text-gray-400"
														>{edge.properties
															.map((p) => p.name)
															.join(', ')}</span
													>
												{:else}
													<span class="text-xs text-gray-400 dark:text-gray-500"
														>{t('common.noProperties')}</span
													>
												{/if}
											</td>
											<td
												class="px-3 py-2 border-b border-gray-100 dark:border-gray-700/50"
											>
												<button
													class="text-blue-500 hover:text-blue-700 text-xs cursor-pointer"
													onclick={() => (alterEdgeName = edge.name)}
													>{t('schema.alter')}</button
												>
												<button
													class="ml-2 text-red-500 hover:text-red-700 text-xs cursor-pointer"
													onclick={() => deleteEdge(edge.name)}
													>{t('common.delete')}</button
												>
											</td>
										</tr>
									{/each}
								</tbody>
							</table>
						</div>
					{/if}
				{:else if activeTab === 'indexes'}
					<div class="flex justify-between items-center mb-4">
						<span class="text-sm text-gray-500 dark:text-gray-400"
							>{indexes.length} {t('sidebar.indexes')}</span
						>
						<button
							class="px-3 py-1.5 bg-blue-500 hover:bg-blue-600 text-white text-sm rounded cursor-pointer"
							onclick={() => {
								showCreateIndex = true;
							}}>+ {t('schema.createIndex')}</button
						>
					</div>
					{#if isLoadingIndexes}
						<div class="flex items-center justify-center p-8">
							<div
								class="animate-spin w-6 h-6 border-2 border-blue-500 border-t-transparent rounded-full"
							></div>
						</div>
					{:else if indexes.length === 0}
						<p class="text-gray-400 dark:text-gray-500 text-center py-8">
							{t('schema.noIndexes')}
						</p>
					{:else}
						<div class="overflow-x-auto">
							<table class="w-full text-sm border-collapse">
								<thead>
									<tr class="bg-gray-50 dark:bg-gray-800/50">
										<th
											class="px-3 py-2 text-left font-medium text-gray-600 dark:text-gray-400 border-b border-gray-200 dark:border-gray-700"
											>{t('common.name')}</th
										>
										<th
											class="px-3 py-2 text-left font-medium text-gray-600 dark:text-gray-400 border-b border-gray-200 dark:border-gray-700"
											>{t('common.type')}</th
										>
										<th
											class="px-3 py-2 text-left font-medium text-gray-600 dark:text-gray-400 border-b border-gray-200 dark:border-gray-700"
											>{t('common.entity')}</th
										>
										<th
											class="px-3 py-2 text-left font-medium text-gray-600 dark:text-gray-400 border-b border-gray-200 dark:border-gray-700"
											>{t('common.fields')}</th
										>
										<th
											class="px-3 py-2 text-left font-medium text-gray-600 dark:text-gray-400 border-b border-gray-200 dark:border-gray-700"
											>{t('common.actions')}</th
										>
									</tr>
								</thead>
								<tbody>
									{#each indexes as idx (idx.name)}
										<tr class="hover:bg-gray-50 dark:hover:bg-gray-800/30">
											<td
												class="px-3 py-2 border-b border-gray-100 dark:border-gray-700/50 font-medium text-gray-800 dark:text-gray-200"
												>{idx.name}</td
											>
											<td
												class="px-3 py-2 border-b border-gray-100 dark:border-gray-700/50 text-gray-700 dark:text-gray-300"
												>{idx.index_type}</td
											>
											<td
												class="px-3 py-2 border-b border-gray-100 dark:border-gray-700/50 text-gray-700 dark:text-gray-300"
												>{idx.entity_type}: {idx.entity_name}</td
											>
											<td
												class="px-3 py-2 border-b border-gray-100 dark:border-gray-700/50 text-gray-700 dark:text-gray-300"
												>{idx.fields?.join(', ')}</td
											>
											<td
												class="px-3 py-2 border-b border-gray-100 dark:border-gray-700/50"
											>
												<button
													class="text-blue-500 hover:text-blue-700 text-xs cursor-pointer"
													onclick={() => rebuildIndex(idx.name)}
													>{t('schema.rebuild')}</button
												>
												<button
													class="ml-2 text-red-500 hover:text-red-700 text-xs cursor-pointer"
													onclick={() => deleteIndex(idx.name)}
													>{t('common.delete')}</button
												>
											</td>
										</tr>
									{/each}
								</tbody>
							</table>
						</div>
					{/if}
				{:else if activeTab === 'functions'}
					<div class="flex justify-between items-center mb-4">
						<span class="text-sm text-gray-500 dark:text-gray-400">
							{functions.length} {t('sidebar.functions')}
						</span>
						<button
							class="px-3 py-1.5 bg-blue-500 hover:bg-blue-600 text-white text-sm rounded cursor-pointer disabled:opacity-50"
							onclick={loadFunctions}
							disabled={isLoadingFunctions}
						>
							{t('common.refresh')}
						</button>
					</div>
					{#if functionsError}
						<p class="text-xs text-red-500 mb-2">{functionsError}</p>
					{/if}
					<div class="grid md:grid-cols-2 gap-2 text-sm mb-4">
						<label class="flex flex-col gap-1 text-xs text-gray-500">
							{t('common.name')}
							<input
								class="px-2 py-1 border border-gray-300 dark:border-gray-600 rounded font-mono bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
								bind:value={newFunctionName}
							/>
						</label>
						<label class="flex flex-col gap-1 text-xs text-gray-500">
							{t('functions.implementation')}
							<input
								class="px-2 py-1 border border-gray-300 dark:border-gray-600 rounded font-mono bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
								placeholder="/usr/lib/graphdb/udf.so"
								bind:value={newFunctionImpl}
							/>
						</label>
					</div>
					<button
						class="px-3 py-1.5 bg-blue-500 hover:bg-blue-600 text-white text-sm rounded cursor-pointer disabled:opacity-50 mb-4"
						onclick={registerFunction}
						disabled={isLoadingFunctions || !newFunctionName || !newFunctionImpl}
					>
						{t('common.create')}
					</button>
					{#if isLoadingFunctions}
						<div class="flex items-center justify-center p-8">
							<div
								class="animate-spin w-6 h-6 border-2 border-blue-500 border-t-transparent rounded-full"
							></div>
						</div>
					{:else if functions.length === 0 && !functionsResult}
						<p class="text-gray-400 dark:text-gray-500 text-center py-8">
							{t('functions.empty')}
						</p>
					{:else}
						<div class="overflow-x-auto mb-4">
							<table class="w-full text-sm border-collapse">
								<thead>
									<tr class="bg-gray-50 dark:bg-gray-800/50">
										<th
											class="px-3 py-2 text-left font-medium text-gray-600 dark:text-gray-400 border-b border-gray-200 dark:border-gray-700"
											>{t('common.name')}</th
										>
										<th
											class="px-3 py-2 text-left font-medium text-gray-600 dark:text-gray-400 border-b border-gray-200 dark:border-gray-700"
											>{t('common.actions')}</th
										>
									</tr>
								</thead>
								<tbody>
									{#each functions as name (name)}
										<tr class="hover:bg-gray-50 dark:hover:bg-gray-800/30">
											<td
												class="px-3 py-2 border-b border-gray-100 dark:border-gray-700/50 font-mono text-gray-800 dark:text-gray-200"
												>{name}</td
											>
											<td
												class="px-3 py-2 border-b border-gray-100 dark:border-gray-700/50"
											>
												<button
													class="text-blue-500 hover:text-blue-700 text-xs cursor-pointer"
													onclick={() => showFunctionInfo(name)}
													>{t('common.detail')}</button
												>
												<button
													class="ml-2 text-red-500 hover:text-red-700 text-xs cursor-pointer"
													onclick={() => unregisterFunction(name)}
													>{t('common.delete')}</button
												>
											</td>
										</tr>
									{/each}
								</tbody>
							</table>
						</div>
					{/if}
					{#if functionsResult}
						<pre
							class="text-xs font-mono bg-gray-50 dark:bg-gray-800/50 p-3 rounded border border-gray-200 dark:border-gray-700 overflow-auto max-h-64 text-gray-700 dark:text-gray-300">{functionsResult}</pre
						>
					{/if}
				{:else if activeTab === 'versions'}
					<div class="grid md:grid-cols-5 gap-2 text-sm mb-3">
						<label class="flex flex-col gap-1 text-xs text-gray-500">
							{t('migration.space')}
							<input
								class="px-2 py-1 border border-gray-300 dark:border-gray-600 rounded font-mono bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
								bind:value={versionSpace}
							/>
						</label>
						<label class="flex flex-col gap-1 text-xs text-gray-500">
							{t('migration.label')}
							<input
								class="px-2 py-1 border border-gray-300 dark:border-gray-600 rounded font-mono bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
								bind:value={versionLabel}
							/>
						</label>
						<label class="flex flex-col gap-1 text-xs text-gray-500">
							{t('migration.fromVersion')}
							<input
								type="number"
								class="px-2 py-1 border border-gray-300 dark:border-gray-600 rounded font-mono bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
								bind:value={versionFrom}
							/>
						</label>
						<label class="flex flex-col gap-1 text-xs text-gray-500">
							{t('migration.toVersion')}
							<input
								type="number"
								class="px-2 py-1 border border-gray-300 dark:border-gray-600 rounded font-mono bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
								bind:value={versionTo}
							/>
						</label>
						<label class="flex items-center gap-1 text-xs text-gray-500 mt-5">
							<input type="checkbox" bind:checked={versionIsEdge} />
							{t('migration.isEdge')}
						</label>
					</div>
					{#if versionsError}
						<p class="text-xs text-red-500 mb-2">{versionsError}</p>
					{/if}
					<div class="flex flex-wrap gap-2 mb-4">
						<button
							class="px-3 py-1.5 bg-blue-500 hover:bg-blue-600 text-white text-sm rounded cursor-pointer disabled:opacity-50"
							onclick={() =>
								runVersionQuery(() =>
									schemaVersionsService.history(
										versionSpace,
										versionLabel,
										versionIsEdge,
									),
								)}
							disabled={isLoadingVersions || !versionSpace || !versionLabel}
						>
							{t('versions.history')}
						</button>
						<button
							class="px-3 py-1.5 bg-gray-100 dark:bg-gray-700/50 text-gray-700 dark:text-gray-200 text-sm rounded cursor-pointer disabled:opacity-50"
							onclick={() =>
								runVersionQuery(() =>
									schemaVersionsService.changes(
										versionSpace,
										versionLabel,
										versionFrom,
										versionTo,
										versionIsEdge,
									),
								)}
							disabled={isLoadingVersions || !versionSpace || !versionLabel}
						>
							{t('versions.changes')}
						</button>
						<button
							class="px-3 py-1.5 bg-amber-500 hover:bg-amber-600 text-white text-sm rounded cursor-pointer disabled:opacity-50"
							onclick={() =>
								runVersionQuery(() =>
									schemaVersionsService.breakingChanges(
										versionSpace,
										versionLabel,
										versionFrom,
										versionTo,
										versionIsEdge,
									),
								)}
							disabled={isLoadingVersions || !versionSpace || !versionLabel}
						>
							{t('versions.breaking')}
						</button>
					</div>
					{#if isLoadingVersions}
						<div class="flex items-center justify-center p-8">
							<div
								class="animate-spin w-6 h-6 border-2 border-blue-500 border-t-transparent rounded-full"
							></div>
						</div>
					{:else if versionsResult}
						<pre
							class="text-xs font-mono bg-gray-50 dark:bg-gray-800/50 p-3 rounded border border-gray-200 dark:border-gray-700 overflow-auto max-h-96 text-gray-700 dark:text-gray-300">{versionsResult}</pre
						>
					{:else}
						<p class="text-gray-400 dark:text-gray-500 text-center py-8">
							{t('versions.hint')}
						</p>
					{/if}
				{:else if activeTab === 'visualization'}
					<SchemaErGraph
						{tags}
						{edgeTypes}
						{isDark}
						onAlterTag={(name) => (alterTagName = name)}
						onAlterEdge={(name) => (alterEdgeName = name)}
					/>
				{/if}
			</div>
		</div>
	</div>
{/if}

<!-- Create Space Modal -->
{#if showCreateSpace}
	<div
		class="fixed inset-0 z-50 flex items-center justify-center"
		role="dialog"
		aria-labelledby="create-space-title"
	>
		<div
			class="absolute inset-0 bg-black/20 cursor-pointer"
			role="button"
			tabindex="0"
			aria-label={t('common.close')}
			onclick={() => (showCreateSpace = false)}
			onkeydown={(e) => {
				if (e.key === 'Enter' || e.key === ' ') showCreateSpace = false;
			}}
		></div>
		<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
		<div
			class="relative bg-white dark:bg-[#1C2333] rounded-lg shadow-lg p-6 w-96"
			role="application"
			tabindex="-1"
			onkeydown={(e) => e.stopPropagation()}
			onclick={(e) => e.stopPropagation()}
		>
			<h3
				id="create-space-title"
				class="font-semibold text-gray-800 dark:text-gray-100 mb-4"
			>
				{t('schema.createSpace')}
			</h3>
			<div class="space-y-3">
				<div>
					<label
						for="space-name"
						class="block text-sm text-gray-600 dark:text-gray-400 mb-1"
						>{t('common.name')}</label
					>
					<input
						id="space-name"
						type="text"
						bind:value={newSpaceName}
						class="w-full px-3 py-2 border border-gray-300 dark:border-gray-600 rounded text-sm focus:outline-none focus:border-blue-500 bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
						placeholder={t('sidebar.spaces')}
					/>
				</div>
				<div>
					<label
						for="space-vid-type"
						class="block text-sm text-gray-600 dark:text-gray-400 mb-1"
						>{t('schema.vidType')}</label
					>
					<select
						id="space-vid-type"
						bind:value={newSpaceVidType}
						class="w-full px-3 py-2 border border-gray-300 dark:border-gray-600 rounded text-sm bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200 focus:outline-none focus:border-blue-500"
					>
						<option value="INT64">INT64</option>
						<option value="FIXED_STRING(32)">FIXED_STRING(32)</option>
					</select>
				</div>
				<div>
					<label
						for="space-partition-num"
						class="block text-sm text-gray-600 dark:text-gray-400 mb-1"
						>{t('schema.partitionNum')}</label
					>
					<input
						id="space-partition-num"
						type="number"
						bind:value={newSpacePartitionNum}
						class="w-full px-3 py-2 border border-gray-300 dark:border-gray-600 rounded text-sm focus:outline-none focus:border-blue-500 bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
					/>
				</div>
				<div>
					<label
						for="space-replica-factor"
						class="block text-sm text-gray-600 dark:text-gray-400 mb-1"
						>{t('schema.replicaFactor')}</label
					>
					<input
						id="space-replica-factor"
						type="number"
						bind:value={newSpaceReplicaFactor}
						class="w-full px-3 py-2 border border-gray-300 dark:border-gray-600 rounded text-sm focus:outline-none focus:border-blue-500 bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
					/>
				</div>
			</div>
			<div class="flex justify-end gap-2 mt-4">
				<button
					class="px-4 py-1.5 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 text-sm rounded cursor-pointer"
					onclick={() => (showCreateSpace = false)}>{t('common.cancel')}</button
				>
				<button
					class="px-4 py-1.5 bg-blue-500 hover:bg-blue-600 text-white text-sm rounded cursor-pointer"
					onclick={createSpace}>{t('common.create')}</button
				>
			</div>
		</div>
	</div>
{/if}

<!-- Create Tag Modal -->
{#if showCreateTag}
	<div
		class="fixed inset-0 z-50 flex items-center justify-center"
		role="dialog"
		aria-labelledby="create-tag-title"
	>
		<div
			class="absolute inset-0 bg-black/20 cursor-pointer"
			role="button"
			tabindex="0"
			aria-label={t('common.close')}
			onclick={() => (showCreateTag = false)}
			onkeydown={(e) => {
				if (e.key === 'Enter' || e.key === ' ') showCreateTag = false;
			}}
		></div>
		<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
		<div
			class="relative bg-white dark:bg-[#1C2333] rounded-lg shadow-lg p-6 w-96 max-h-[80vh] overflow-y-auto"
			role="application"
			tabindex="-1"
			onkeydown={(e) => e.stopPropagation()}
			onclick={(e) => e.stopPropagation()}
		>
			<h3
				id="create-tag-title"
				class="font-semibold text-gray-800 dark:text-gray-100 mb-4"
			>
				{t('schema.createTag')}
			</h3>
			<div class="space-y-3">
				<div>
					<label
						for="tag-name"
						class="block text-sm text-gray-600 dark:text-gray-400 mb-1"
						>{t('common.name')}</label
					>
					<input
						id="tag-name"
						type="text"
						bind:value={newTagName}
						class="w-full px-3 py-2 border border-gray-300 dark:border-gray-600 rounded text-sm focus:outline-none focus:border-blue-500 bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
						placeholder={t('sidebar.tags')}
					/>
				</div>
				<div>
					<!-- svelte-ignore a11y_label_has_associated_control -->
					<label class="block text-sm text-gray-600 dark:text-gray-400 mb-1"
						>{t('common.properties')}</label
					>
					{#each newTagProps as prop, i (i)}
						<div class="flex gap-2 mb-2 items-start">
							<input
								type="text"
								bind:value={prop.name}
								class="flex-1 px-2 py-1.5 border border-gray-300 dark:border-gray-600 rounded text-sm bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
								placeholder={t('common.name')}
							/>
							<select
								bind:value={prop.data_type}
								class="px-2 py-1.5 border border-gray-300 dark:border-gray-600 rounded text-sm bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
							>
								{#each dataTypes as dt (dt)}
									<option value={dt}>{dt}</option>
								{/each}
							</select>
							<button
								class="text-red-400 hover:text-red-600 cursor-pointer px-1"
								onclick={() => removeProp(newTagProps, i)}>✕</button
							>
						</div>
					{/each}
					<button
						class="text-blue-500 hover:text-blue-700 text-xs cursor-pointer"
						onclick={() => addProp(newTagProps)}
						>+ {t('common.addProperty')}</button
					>
				</div>
			</div>
			<div class="flex justify-end gap-2 mt-4">
				<button
					class="px-4 py-1.5 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 text-sm rounded cursor-pointer"
					onclick={() => (showCreateTag = false)}>{t('common.cancel')}</button
				>
				<button
					class="px-4 py-1.5 bg-blue-500 hover:bg-blue-600 text-white text-sm rounded cursor-pointer"
					onclick={createTag}>{t('common.create')}</button
				>
			</div>
		</div>
	</div>
{/if}

<!-- Create Edge Modal -->
{#if showCreateEdge}
	<div
		class="fixed inset-0 z-50 flex items-center justify-center"
		role="dialog"
		aria-labelledby="create-edge-title"
	>
		<div
			class="absolute inset-0 bg-black/20 cursor-pointer"
			role="button"
			tabindex="0"
			aria-label={t('common.close')}
			onclick={() => (showCreateEdge = false)}
			onkeydown={(e) => {
				if (e.key === 'Enter' || e.key === ' ') showCreateEdge = false;
			}}
		></div>
		<!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
		<div
			class="relative bg-white dark:bg-[#1C2333] rounded-lg shadow-lg p-6 w-96 max-h-[80vh] overflow-y-auto"
			role="application"
			tabindex="-1"
			onkeydown={(e) => e.stopPropagation()}
			onclick={(e) => e.stopPropagation()}
		>
			<h3
				id="create-edge-title"
				class="font-semibold text-gray-800 dark:text-gray-100 mb-4"
			>
				{t('schema.createEdge')}
			</h3>
			<div class="space-y-3">
				<div>
					<label
						for="edge-name"
						class="block text-sm text-gray-600 dark:text-gray-400 mb-1"
						>{t('common.name')}</label
					>
					<input
						id="edge-name"
						type="text"
						bind:value={newEdgeName}
						class="w-full px-3 py-2 border border-gray-300 dark:border-gray-600 rounded text-sm bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
						placeholder={t('sidebar.edges')}
					/>
				</div>
				<div>
					<!-- svelte-ignore a11y_label_has_associated_control -->
					<label class="block text-sm text-gray-600 dark:text-gray-400 mb-1"
						>{t('common.properties')}</label
					>
					{#each newEdgeProps as prop, i (i)}
						<div class="flex gap-2 mb-2">
							<input
								type="text"
								bind:value={prop.name}
								class="flex-1 px-2 py-1.5 border border-gray-300 dark:border-gray-600 rounded text-sm bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
								placeholder={t('common.name')}
							/>
							<select
								bind:value={prop.data_type}
								class="px-2 py-1.5 border border-gray-300 dark:border-gray-600 rounded text-sm bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
							>
								{#each dataTypes as dt (dt)}
									<option value={dt}>{dt}</option>
								{/each}
							</select>
							<button
								class="text-red-400 hover:text-red-600 cursor-pointer"
								onclick={() => removeProp(newEdgeProps, i)}>✕</button
							>
						</div>
					{/each}
					<button
						class="text-blue-500 hover:text-blue-700 text-xs cursor-pointer"
						onclick={() => addProp(newEdgeProps)}
						>+ {t('common.addProperty')}</button
					>
				</div>
			</div>
			<div class="flex justify-end gap-2 mt-4">
				<button
					class="px-4 py-1.5 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 text-sm rounded cursor-pointer"
					onclick={() => (showCreateEdge = false)}>{t('common.cancel')}</button
				>
				<button
					class="px-4 py-1.5 bg-blue-500 hover:bg-blue-600 text-white text-sm rounded cursor-pointer"
					onclick={createEdge}>{t('common.create')}</button
				>
			</div>
		</div>
	</div>
{/if}

<!-- Create Index Modal -->
{#if showCreateIndex}
	<!-- svelte-ignore a11y_click_events_have_key_events -->
	<div
		role="presentation"
		class="fixed inset-0 z-50 flex items-center justify-center"
		onclick={() => (showCreateIndex = false)}
	>
		<div class="absolute inset-0 bg-black/20"></div>
		<!-- svelte-ignore a11y_no_static_element_interactions -->
		<div
			class="relative bg-white dark:bg-[#1C2333] rounded-lg shadow-lg p-6 w-96"
			onclick={(e) => e.stopPropagation()}
		>
			<h3 class="font-semibold text-gray-800 dark:text-gray-100 mb-4">
				{t('schema.createIndex')}
			</h3>
			<div class="space-y-3">
				<div>
					<label
						for="index-name"
						class="block text-sm text-gray-600 dark:text-gray-400 mb-1"
						>{t('common.name')}</label
					>
					<input
						id="index-name"
						type="text"
						bind:value={newIndexName}
						class="w-full px-3 py-2 border border-gray-300 dark:border-gray-600 rounded text-sm bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
						placeholder={t('sidebar.indexes')}
					/>
				</div>
				<div>
					<label
						for="index-entity-type"
						class="block text-sm text-gray-600 dark:text-gray-400 mb-1"
						>{t('common.entityType')}</label
					>
					<select
						id="index-entity-type"
						bind:value={newIndexEntityType}
						class="w-full px-3 py-2 border border-gray-300 dark:border-gray-600 rounded text-sm bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
					>
						<option value="TAG">TAG</option>
						<option value="EDGE">EDGE</option>
					</select>
				</div>
				<div>
					<label
						for="index-entity-name"
						class="block text-sm text-gray-600 dark:text-gray-400 mb-1"
						>{t('common.entityName')}</label
					>
					<input
						id="index-entity-name"
						type="text"
						bind:value={newIndexEntityName}
						class="w-full px-3 py-2 border border-gray-300 dark:border-gray-600 rounded text-sm bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
						placeholder="{t('sidebar.tags')} / {t('sidebar.edges')}"
					/>
				</div>
				<div>
					<label
						for="index-fields"
						class="block text-sm text-gray-600 dark:text-gray-400 mb-1"
						>{t('common.fields')}</label
					>
					<input
						id="index-fields"
						type="text"
						bind:value={newIndexFields}
						class="w-full px-3 py-2 border border-gray-300 dark:border-gray-600 rounded text-sm bg-white dark:bg-[#1C2333] text-gray-800 dark:text-gray-200"
						placeholder={t('schema.fieldPlaceholder')}
					/>
				</div>
			</div>
			<div class="flex justify-end gap-2 mt-4">
				<button
					class="px-4 py-1.5 border border-gray-300 dark:border-gray-600 bg-white dark:bg-[#1C2333] hover:bg-gray-50 dark:hover:bg-gray-700/50 text-gray-700 dark:text-gray-300 text-sm rounded cursor-pointer"
					onclick={() => (showCreateIndex = false)}>{t('common.cancel')}</button
				>
				<button
					class="px-4 py-1.5 bg-blue-500 hover:bg-blue-600 text-white text-sm rounded cursor-pointer"
					onclick={createIndex}>{t('common.create')}</button
				>
			</div>
		</div>
	</div>
{/if}

<SchemaAlterModal
	title={alterTagName ? `${t('schema.alterTag')}: ${alterTagName}` : ''}
	properties={tags.find((item) => item.name === alterTagName)?.properties ?? []}
	open={alterTagName !== null}
	busy={alterBusy}
	onClose={() => (alterTagName = null)}
	onSubmit={submitAlterTag}
/>
<SchemaAlterModal
	title={alterEdgeName ? `${t('schema.alterEdge')}: ${alterEdgeName}` : ''}
	properties={edgeTypes.find((item) => item.name === alterEdgeName)?.properties ?? []}
	open={alterEdgeName !== null}
	busy={alterBusy}
	onClose={() => (alterEdgeName = null)}
	onSubmit={submitAlterEdge}
/>

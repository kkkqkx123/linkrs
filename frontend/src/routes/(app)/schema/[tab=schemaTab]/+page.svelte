<script lang="ts">
	import { onMount } from 'svelte';
	import { t, type MessageKey } from '$i18n';
	import PageHeader from '$components/layout/PageHeader.svelte';
	import { X } from 'lucide-svelte';
	import { functionsService } from '$services/functions';
	import { schemaVersionsService } from '$services/schemaVersions';
	import { schemaStore } from '$stores/schema';
	import { theme } from '$stores/theme';
	import { DATA_TYPE_LABELS } from '$config/constants';
	import PageSkeleton from '$components/common/PageSkeleton.svelte';
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
	<div class="ui-page">
		<PageHeader title={t('schema.title')} />

		<!-- Space Selector -->
		<div class="ui-card shadow-sm px-6 py-4">
			<div class="flex items-center gap-4">
				<span class="text-sm font-medium text-muted"
					>{t('sidebar.spaces')}:</span
				>
				<select
					class="ui-input"
					value={currentSpace || ''}
					onchange={(e) => selectSpace((e.target as HTMLSelectElement).value)}
				>
					<option value="">-- {t('common.select')} --</option>
					{#each spaces as s (s.name)}
						<option value={s.name}>{s.name}</option>
					{/each}
				</select>
				<button
					class="ui-btn ui-btn-primary"
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
			class="ui-card ui-card-flush shadow-sm flex flex-col flex-1 overflow-hidden"
		>
			<div class="flex border-b border-line">
				{#each TABS as tab (tab.id)}
					<a
						href={`/schema/${tab.id}`}
						class="ui-tab {activeTab === tab.id ? 'ui-tab-active' : ''}"
						aria-current={activeTab === tab.id ? 'page' : undefined}
					>
						{t(tab.label)}
					</a>
				{/each}
			</div>

			<div class="flex-1 overflow-auto p-5">
				{#if activeTab === 'spaces'}
					{#if isLoadingSpaces}
						<div class="ui-empty">
							<div class="ui-spinner"></div>
						</div>
					{:else if spaces.length === 0}
						<p class="text-muted text-center py-8">
							{t('schema.noSpaces')}
						</p>
					{:else}
						<div class="overflow-x-auto">
							<table class="ui-table w-full border-collapse">
								<thead>
									<tr class="bg-secondary">
										<th>{t('common.name')}</th>
										<th>{t('schema.vidType')}</th>
										<th>{t('common.actions')}</th>
									</tr>
								</thead>
								<tbody>
									{#each spaces as space (space.name)}
										<tr
											class="hover:bg-hover {currentSpace ===
											space.name
												? 'bg-info-light'
												: ''}"
										>
											<td class="font-medium">{space.name}</td>
											<td class="font-mono">{space.vid_type}</td>
											<td>
												<button
													class="ui-link-danger text-xs"
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
						<span class="text-sm text-muted"
							>{tags.length} {t('sidebar.tags')}</span
						>
						<button
							class="ui-btn ui-btn-primary"
							onclick={() => {
								showCreateTag = true;
							}}>+ {t('schema.createTag')}</button
						>
					</div>
					{#if isLoadingTags}
						<div class="ui-empty">
							<div class="ui-spinner"></div>
						</div>
					{:else if tags.length === 0}
						<p class="text-muted text-center py-8">
							{t('schema.noTags')}
						</p>
					{:else}
						<div class="overflow-x-auto">
							<table class="ui-table w-full border-collapse">
								<thead>
									<tr class="bg-secondary">
										<th>{t('common.name')}</th>
										<th>{t('common.properties')}</th>
										<th>{t('common.actions')}</th>
									</tr>
								</thead>
								<tbody>
									{#each tags as tag (tag.name)}
										<tr>
											<td class="font-medium">{tag.name}</td>
											<td>
												{#if tag.properties?.length}
													<span class="text-xs text-muted"
														>{tag.properties
															.map((p) => p.name)
															.join(', ')}</span
													>
												{:else}
													<span class="text-xs text-muted"
														>{t('common.noProperties')}</span
													>
												{/if}
											</td>
											<td>
												<button
													class="ui-link text-xs"
													onclick={() => (alterTagName = tag.name)}
													>{t('schema.alter')}</button
												>
												<button
													class="ml-2 ui-link-danger text-xs"
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
						<span class="text-sm text-muted"
							>{edgeTypes.length} {t('sidebar.edges')}</span
						>
						<button
							class="ui-btn ui-btn-primary"
							onclick={() => {
								showCreateEdge = true;
							}}>+ {t('schema.createEdge')}</button
						>
					</div>
					{#if isLoadingEdgeTypes}
						<div class="ui-empty">
							<div class="ui-spinner"></div>
						</div>
					{:else if edgeTypes.length === 0}
						<p class="text-muted text-center py-8">
							{t('schema.noEdges')}
						</p>
					{:else}
						<div class="overflow-x-auto">
							<table class="ui-table w-full border-collapse">
								<thead>
									<tr class="bg-secondary">
										<th>{t('common.name')}</th>
										<th>{t('common.properties')}</th>
										<th>{t('common.actions')}</th>
									</tr>
								</thead>
								<tbody>
									{#each edgeTypes as edge (edge.name)}
										<tr>
											<td class="font-medium">{edge.name}</td>
											<td>
												{#if edge.properties?.length}
													<span class="text-xs text-muted"
														>{edge.properties
															.map((p) => p.name)
															.join(', ')}</span
													>
												{:else}
													<span class="text-xs text-muted"
														>{t('common.noProperties')}</span
													>
												{/if}
											</td>
											<td>
												<button
													class="ui-link text-xs"
													onclick={() => (alterEdgeName = edge.name)}
													>{t('schema.alter')}</button
												>
												<button
													class="ml-2 ui-link-danger text-xs"
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
						<span class="text-sm text-muted"
							>{indexes.length} {t('sidebar.indexes')}</span
						>
						<button
							class="ui-btn ui-btn-primary"
							onclick={() => {
								showCreateIndex = true;
							}}>+ {t('schema.createIndex')}</button
						>
					</div>
					{#if isLoadingIndexes}
						<div class="ui-empty">
							<div class="ui-spinner"></div>
						</div>
					{:else if indexes.length === 0}
						<p class="text-muted text-center py-8">
							{t('schema.noIndexes')}
						</p>
					{:else}
						<div class="overflow-x-auto">
							<table class="ui-table w-full border-collapse">
								<thead>
									<tr class="bg-secondary">
										<th>{t('common.name')}</th>
										<th>{t('common.type')}</th>
										<th>{t('common.entity')}</th>
										<th>{t('common.fields')}</th>
										<th>{t('common.actions')}</th>
									</tr>
								</thead>
								<tbody>
									{#each indexes as idx (idx.name)}
										<tr>
											<td class="font-medium">{idx.name}</td>
											<td>{idx.index_type}</td>
											<td>{idx.entity_type}: {idx.entity_name}</td>
											<td>{idx.fields?.join(', ')}</td>
											<td>
												<button
													class="ui-link text-xs"
													onclick={() => rebuildIndex(idx.name)}
													>{t('schema.rebuild')}</button
												>
												<button
													class="ml-2 ui-link-danger text-xs"
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
						<span class="text-sm text-muted">
							{functions.length}
							{t('sidebar.functions')}
						</span>
						<button
							class="ui-btn ui-btn-primary"
							onclick={loadFunctions}
							disabled={isLoadingFunctions}
						>
							{t('common.refresh')}
						</button>
					</div>
					{#if functionsError}
						<p class="text-xs text-error mb-2">{functionsError}</p>
					{/if}
					<div class="grid md:grid-cols-2 gap-2 text-sm mb-4">
						<label class="flex flex-col gap-1 text-xs text-muted">
							{t('common.name')}
							<input
								class="ui-input ui-input-sm font-mono"
								bind:value={newFunctionName}
							/>
						</label>
						<label class="flex flex-col gap-1 text-xs text-muted">
							{t('functions.implementation')}
							<input
								class="ui-input ui-input-sm font-mono"
								placeholder="/usr/lib/linkrs/udf.so"
								bind:value={newFunctionImpl}
							/>
						</label>
					</div>
					<button
						class="ui-btn ui-btn-primary mb-4"
						onclick={registerFunction}
						disabled={isLoadingFunctions ||
							!newFunctionName ||
							!newFunctionImpl}
					>
						{t('common.create')}
					</button>
					{#if isLoadingFunctions}
						<div class="ui-empty">
							<div class="ui-spinner"></div>
						</div>
					{:else if functions.length === 0 && !functionsResult}
						<p class="text-muted text-center py-8">
							{t('functions.empty')}
						</p>
					{:else}
						<div class="overflow-x-auto mb-4">
							<table class="ui-table w-full border-collapse">
								<thead>
									<tr class="bg-secondary">
										<th>{t('common.name')}</th>
										<th>{t('common.actions')}</th>
									</tr>
								</thead>
								<tbody>
									{#each functions as name (name)}
										<tr>
											<td>{name}</td>
											<td>
												<button
													class="ui-link text-xs"
													onclick={() => showFunctionInfo(name)}
													>{t('common.detail')}</button
												>
												<button
													class="ml-2 ui-link-danger text-xs"
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
						<pre class="ui-pre max-h-64">{functionsResult}</pre>
					{/if}
				{:else if activeTab === 'versions'}
					<div class="grid md:grid-cols-5 gap-2 text-sm mb-3">
						<label class="flex flex-col gap-1 text-xs text-muted">
							{t('migration.space')}
							<input
								class="ui-input ui-input-sm font-mono"
								bind:value={versionSpace}
							/>
						</label>
						<label class="flex flex-col gap-1 text-xs text-muted">
							{t('migration.label')}
							<input
								class="ui-input ui-input-sm font-mono"
								bind:value={versionLabel}
							/>
						</label>
						<label class="flex flex-col gap-1 text-xs text-muted">
							{t('migration.fromVersion')}
							<input
								type="number"
								class="ui-input ui-input-sm font-mono"
								bind:value={versionFrom}
							/>
						</label>
						<label class="flex flex-col gap-1 text-xs text-muted">
							{t('migration.toVersion')}
							<input
								type="number"
								class="ui-input ui-input-sm font-mono"
								bind:value={versionTo}
							/>
						</label>
						<label class="flex items-center gap-1 text-xs text-muted mt-5">
							<input type="checkbox" bind:checked={versionIsEdge} />
							{t('migration.isEdge')}
						</label>
					</div>
					{#if versionsError}
						<p class="text-xs text-error mb-2">{versionsError}</p>
					{/if}
					<div class="flex flex-wrap gap-2 mb-4">
						<button
							class="ui-btn ui-btn-primary"
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
							class="px-3 py-1.5 bg-secondary text-ink text-sm rounded cursor-pointer disabled:opacity-50"
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
							class="ui-btn ui-btn-warn ui-btn-sm"
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
						<div class="ui-empty">
							<div class="ui-spinner"></div>
						</div>
					{:else if versionsResult}
						<pre class="ui-pre">{versionsResult}</pre>
					{:else}
						<p class="text-muted text-center py-8">
							{t('versions.hint')}
						</p>
					{/if}
				{:else if activeTab === 'visualization'}
					{#await import('$components/business/SchemaErGraph.svelte')}
						<div class="ui-empty">
							<div class="ui-spinner"></div>
						</div>
					{:then { default: SchemaErGraph }}
						<SchemaErGraph
							{tags}
							{edgeTypes}
							{isDark}
							onAlterTag={(name: string) => (alterTagName = name)}
							onAlterEdge={(name: string) => (alterEdgeName = name)}
						/>
					{/await}
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
			class="absolute inset-0 bg-mask cursor-pointer"
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
			class="relative ui-card shadow-lg p-6 w-96"
			role="application"
			tabindex="-1"
			onkeydown={(e) => e.stopPropagation()}
			onclick={(e) => e.stopPropagation()}
		>
			<h3
				id="create-space-title"
				class="font-semibold text-ink mb-4"
			>
				{t('schema.createSpace')}
			</h3>
			<div class="space-y-3">
				<div>
					<label
						for="space-name"
						class="block text-sm text-muted mb-1"
						>{t('common.name')}</label
					>
					<input
						id="space-name"
						type="text"
						bind:value={newSpaceName}
						class="ui-input w-full"
						placeholder={t('sidebar.spaces')}
					/>
				</div>
				<div>
					<label
						for="space-vid-type"
						class="block text-sm text-muted mb-1"
						>{t('schema.vidType')}</label
					>
					<select
						id="space-vid-type"
						bind:value={newSpaceVidType}
						class="ui-input w-full focus:outline-none focus:border-info/40"
					>
						<option value="INT64">INT64</option>
						<option value="FIXED_STRING(32)">FIXED_STRING(32)</option>
					</select>
				</div>
				<div>
					<label
						for="space-partition-num"
						class="block text-sm text-muted mb-1"
						>{t('schema.partitionNum')}</label
					>
					<input
						id="space-partition-num"
						type="number"
						bind:value={newSpacePartitionNum}
						class="ui-input w-full"
					/>
				</div>
				<div>
					<label
						for="space-replica-factor"
						class="block text-sm text-muted mb-1"
						>{t('schema.replicaFactor')}</label
					>
					<input
						id="space-replica-factor"
						type="number"
						bind:value={newSpaceReplicaFactor}
						class="ui-input w-full"
					/>
				</div>
			</div>
			<div class="flex justify-end gap-2 mt-4">
				<button
					class="ui-btn ui-btn-ghost"
					onclick={() => (showCreateSpace = false)}>{t('common.cancel')}</button
				>
				<button class="ui-btn ui-btn-primary" onclick={createSpace}
					>{t('common.create')}</button
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
			class="absolute inset-0 bg-mask cursor-pointer"
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
			class="relative ui-card shadow-lg p-6 w-96 max-h-[80vh] overflow-y-auto"
			role="application"
			tabindex="-1"
			onkeydown={(e) => e.stopPropagation()}
			onclick={(e) => e.stopPropagation()}
		>
			<h3
				id="create-tag-title"
				class="font-semibold text-ink mb-4"
			>
				{t('schema.createTag')}
			</h3>
			<div class="space-y-3">
				<div>
					<label
						for="tag-name"
						class="block text-sm text-muted mb-1"
						>{t('common.name')}</label
					>
					<input
						id="tag-name"
						type="text"
						bind:value={newTagName}
						class="ui-input w-full"
						placeholder={t('sidebar.tags')}
					/>
				</div>
				<div>
					<!-- svelte-ignore a11y_label_has_associated_control -->
					<label class="block text-sm text-muted mb-1"
						>{t('common.properties')}</label
					>
					{#each newTagProps as prop, i (i)}
						<div class="flex gap-2 mb-2 items-start">
							<input
								type="text"
								bind:value={prop.name}
								class="ui-input"
								placeholder={t('common.name')}
							/>
							<select bind:value={prop.data_type} class="ui-input">
								{#each dataTypes as dt (dt)}
									<option value={dt}>{dt}</option>
								{/each}
							</select>
							<button
								class="ui-link-danger px-1"
								onclick={() => removeProp(newTagProps, i)}><X size={12} /></button
							>
						</div>
					{/each}
					<button class="ui-link text-xs" onclick={() => addProp(newTagProps)}
						>+ {t('common.addProperty')}</button
					>
				</div>
			</div>
			<div class="flex justify-end gap-2 mt-4">
				<button
					class="ui-btn ui-btn-ghost"
					onclick={() => (showCreateTag = false)}>{t('common.cancel')}</button
				>
				<button class="ui-btn ui-btn-primary" onclick={createTag}
					>{t('common.create')}</button
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
			class="absolute inset-0 bg-mask cursor-pointer"
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
			class="relative ui-card shadow-lg p-6 w-96 max-h-[80vh] overflow-y-auto"
			role="application"
			tabindex="-1"
			onkeydown={(e) => e.stopPropagation()}
			onclick={(e) => e.stopPropagation()}
		>
			<h3
				id="create-edge-title"
				class="font-semibold text-ink mb-4"
			>
				{t('schema.createEdge')}
			</h3>
			<div class="space-y-3">
				<div>
					<label
						for="edge-name"
						class="block text-sm text-muted mb-1"
						>{t('common.name')}</label
					>
					<input
						id="edge-name"
						type="text"
						bind:value={newEdgeName}
						class="ui-input w-full"
						placeholder={t('sidebar.edges')}
					/>
				</div>
				<div>
					<!-- svelte-ignore a11y_label_has_associated_control -->
					<label class="block text-sm text-muted mb-1"
						>{t('common.properties')}</label
					>
					{#each newEdgeProps as prop, i (i)}
						<div class="flex gap-2 mb-2">
							<input
								type="text"
								bind:value={prop.name}
								class="ui-input"
								placeholder={t('common.name')}
							/>
							<select bind:value={prop.data_type} class="ui-input">
								{#each dataTypes as dt (dt)}
									<option value={dt}>{dt}</option>
								{/each}
							</select>
							<button
								class="ui-link-danger"
								onclick={() => removeProp(newEdgeProps, i)}><X size={12} /></button
							>
						</div>
					{/each}
					<button class="ui-link text-xs" onclick={() => addProp(newEdgeProps)}
						>+ {t('common.addProperty')}</button
					>
				</div>
			</div>
			<div class="flex justify-end gap-2 mt-4">
				<button
					class="ui-btn ui-btn-ghost"
					onclick={() => (showCreateEdge = false)}>{t('common.cancel')}</button
				>
				<button class="ui-btn ui-btn-primary" onclick={createEdge}
					>{t('common.create')}</button
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
		<div class="absolute inset-0 bg-mask"></div>
		<!-- svelte-ignore a11y_no_static_element_interactions -->
		<div
			class="relative ui-card shadow-lg p-6 w-96"
			onclick={(e) => e.stopPropagation()}
		>
			<h3 class="text-lg font-semibold text-ink mb-4">
				{t('schema.createIndex')}
			</h3>
			<div class="space-y-3">
				<div>
					<label
						for="index-name"
						class="block text-sm text-muted mb-1"
						>{t('common.name')}</label
					>
					<input
						id="index-name"
						type="text"
						bind:value={newIndexName}
						class="ui-input w-full"
						placeholder={t('sidebar.indexes')}
					/>
				</div>
				<div>
					<label
						for="index-entity-type"
						class="block text-sm text-muted mb-1"
						>{t('common.entityType')}</label
					>
					<select
						id="index-entity-type"
						bind:value={newIndexEntityType}
						class="ui-input w-full"
					>
						<option value="TAG">TAG</option>
						<option value="EDGE">EDGE</option>
					</select>
				</div>
				<div>
					<label
						for="index-entity-name"
						class="block text-sm text-muted mb-1"
						>{t('common.entityName')}</label
					>
					<input
						id="index-entity-name"
						type="text"
						bind:value={newIndexEntityName}
						class="ui-input w-full"
						placeholder="{t('sidebar.tags')} / {t('sidebar.edges')}"
					/>
				</div>
				<div>
					<label
						for="index-fields"
						class="block text-sm text-muted mb-1"
						>{t('common.fields')}</label
					>
					<input
						id="index-fields"
						type="text"
						bind:value={newIndexFields}
						class="ui-input w-full"
						placeholder={t('schema.fieldPlaceholder')}
					/>
				</div>
			</div>
			<div class="flex justify-end gap-2 mt-4">
				<button
					class="ui-btn ui-btn-ghost"
					onclick={() => (showCreateIndex = false)}>{t('common.cancel')}</button
				>
				<button class="ui-btn ui-btn-primary" onclick={createIndex}
					>{t('common.create')}</button
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
	properties={edgeTypes.find((item) => item.name === alterEdgeName)
		?.properties ?? []}
	open={alterEdgeName !== null}
	busy={alterBusy}
	onClose={() => (alterEdgeName = null)}
	onSubmit={submitAlterEdge}
/>

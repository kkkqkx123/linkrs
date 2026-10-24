import { writable } from 'svelte/store';
import { schemaService } from '$services/schema';
import { queryService } from '$services/query';
import type {
	Space,
	Tag,
	EdgeType,
	CreateTagParams,
	CreateEdgeTypeParams,
	CreateIndexParams,
	UpdateTagParams,
	UpdateEdgeTypeParams,
} from '$types/schema';
import type { components } from '$lib/api/schema';

type IndexInfo = components['schemas']['IndexInfo'];

export interface CreateSpaceParams {
	name: string;
	vidType: 'INT64' | 'FIXED_STRING(32)';
	partitionNum: number;
	replicaFactor: number;
}

interface SchemaState {
	spaces: Space[];
	isLoadingSpaces: boolean;
	currentSpace: string | null;
	tags: Tag[];
	isLoadingTags: boolean;
	edgeTypes: EdgeType[];
	isLoadingEdgeTypes: boolean;
	indexes: IndexInfo[];
	isLoadingIndexes: boolean;
}

function persistCurrentSpace(name: string | null) {
	if (name) localStorage.setItem('schema-current-space', name);
	else localStorage.removeItem('schema-current-space');
}

function createSchemaStore() {
	const savedSpace = localStorage.getItem('schema-current-space');
	const { subscribe, update } = writable<SchemaState>({
		spaces: [],
		isLoadingSpaces: false,
		currentSpace: savedSpace,
		tags: [],
		isLoadingTags: false,
		edgeTypes: [],
		isLoadingEdgeTypes: false,
		indexes: [],
		isLoadingIndexes: false,
	});

	const store = {
		subscribe,
		fetchSpaces: async () => {
			update((s) => ({ ...s, isLoadingSpaces: true }));
			try {
				const response = await schemaService.spaces.list();
				const spaces = Array.isArray(response)
					? response
					: (response as { data?: Space[] }).data || [];
				update((s) => {
					const newState = { ...s, spaces, isLoadingSpaces: false };
					if (!newState.currentSpace && spaces.length > 0) {
						newState.currentSpace = spaces[0].name;
						persistCurrentSpace(spaces[0].name);
					}
					return newState;
				});
			} catch (err) {
				console.error('Fetch spaces error:', err);
				update((s) => ({ ...s, isLoadingSpaces: false }));
			}
		},
		createSpace: async (params: CreateSpaceParams) => {
			const vidTypeStr =
				params.vidType === 'FIXED_STRING(32)' ? 'FIXED_STRING(32)' : 'INT64';
			const query = `CREATE SPACE IF NOT EXISTS ${params.name} (vid_type = ${vidTypeStr}, partition_num = ${params.partitionNum}, replica_factor = ${params.replicaFactor})`;
			await queryService.execute({ query });
			await store.fetchSpaces();
		},
		deleteSpace: async (name: string) => {
			const query = `DROP SPACE IF EXISTS ${name}`;
			await queryService.execute({ query });
			await store.fetchSpaces();
		},
		setCurrentSpace: (name: string | null) => {
			update((s) => ({ ...s, currentSpace: name }));
			persistCurrentSpace(name);
		},
		fetchTags: async (spaceName: string) => {
			update((s) => ({ ...s, isLoadingTags: true }));
			try {
				const response = await schemaService.tags.list(spaceName);
				const tags = Array.isArray(response)
					? response
					: (response as { data?: Tag[] }).data || [];
				update((s) => ({ ...s, tags, isLoadingTags: false }));
			} catch (err) {
				console.error('Fetch tags error:', err);
				update((s) => ({ ...s, isLoadingTags: false }));
			}
		},
		createTag: async (spaceName: string, params: CreateTagParams) => {
			await schemaService.tags.create(spaceName, params);
			await store.fetchTags(spaceName);
		},
		updateTag: async (
			spaceName: string,
			tagName: string,
			params: UpdateTagParams,
		) => {
			const queryParts: string[] = [];
			if (params.add_properties?.length) {
				const addProps = params.add_properties
					.map(
						(p) =>
							`${p.name} ${p.data_type}${p.default_value ? ` DEFAULT ${p.default_value}` : ''}`,
					)
					.join(', ');
				queryParts.push(`ADD (${addProps})`);
			}
			if (params.drop_properties?.length)
				queryParts.push(`DROP (${params.drop_properties.join(', ')})`);
			if (queryParts.length > 0) {
				await queryService.execute({
					query: `ALTER TAG ${tagName} ${queryParts.join(' ')}`,
				});
				await store.fetchTags(spaceName);
			}
		},
		deleteTag: async (spaceName: string, tagName: string) => {
			await schemaService.tags.delete(spaceName, tagName);
			await store.fetchTags(spaceName);
		},
		fetchEdgeTypes: async (spaceName: string) => {
			update((s) => ({ ...s, isLoadingEdgeTypes: true }));
			try {
				const response = await schemaService.edgeTypes.list(spaceName);
				const edgeTypes = Array.isArray(response)
					? response
					: (response as { data?: EdgeType[] }).data || [];
				update((s) => ({ ...s, edgeTypes, isLoadingEdgeTypes: false }));
			} catch (err) {
				console.error('Fetch edge types error:', err);
				update((s) => ({ ...s, isLoadingEdgeTypes: false }));
			}
		},
		createEdgeType: async (spaceName: string, params: CreateEdgeTypeParams) => {
			await schemaService.edgeTypes.create(spaceName, params);
			await store.fetchEdgeTypes(spaceName);
		},
		updateEdgeType: async (
			spaceName: string,
			edgeName: string,
			params: UpdateEdgeTypeParams,
		) => {
			const queryParts: string[] = [];
			if (params.add_properties?.length) {
				const addProps = params.add_properties
					.map(
						(p) =>
							`${p.name} ${p.data_type}${p.default_value ? ` DEFAULT ${p.default_value}` : ''}`,
					)
					.join(', ');
				queryParts.push(`ADD (${addProps})`);
			}
			if (params.drop_properties?.length)
				queryParts.push(`DROP (${params.drop_properties.join(', ')})`);
			if (queryParts.length > 0) {
				await queryService.execute({
					query: `ALTER EDGE ${edgeName} ${queryParts.join(' ')}`,
				});
				await store.fetchEdgeTypes(spaceName);
			}
		},
		deleteEdgeType: async (spaceName: string, edgeName: string) => {
			await schemaService.edgeTypes.delete(spaceName, edgeName);
			await store.fetchEdgeTypes(spaceName);
		},
		fetchIndexes: async (spaceName: string) => {
			update((s) => ({ ...s, isLoadingIndexes: true }));
			try {
				const response = await schemaService.indexes.list(spaceName);
				const indexes = Array.isArray(response)
					? response
					: (response as { data?: IndexInfo[] }).data || [];
				update((s) => ({ ...s, indexes, isLoadingIndexes: false }));
			} catch (err) {
				console.error('Fetch indexes error:', err);
				update((s) => ({ ...s, isLoadingIndexes: false }));
			}
		},
		createIndex: async (spaceName: string, params: CreateIndexParams) => {
			await schemaService.indexes.create(spaceName, params);
			await store.fetchIndexes(spaceName);
		},
		deleteIndex: async (spaceName: string, indexName: string) => {
			await schemaService.indexes.delete(spaceName, indexName);
			await store.fetchIndexes(spaceName);
		},
		rebuildIndex: async (spaceName: string, indexName: string) => {
			await schemaService.indexes.rebuild(spaceName, indexName);
			await store.fetchIndexes(spaceName);
		},
	};
	return store;
}

export const schemaStore = createSchemaStore();

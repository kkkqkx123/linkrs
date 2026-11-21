import { writable } from 'svelte/store';
import { storage } from '$utils/storage';
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

const CURRENT_SPACE_KEY = 'schema.current-space';

function persistCurrentSpace(name: string | null) {
	if (name) storage.set(CURRENT_SPACE_KEY, name);
	else storage.remove(CURRENT_SPACE_KEY);
}

function createSchemaStore() {
	const savedSpace = storage.get<string>(CURRENT_SPACE_KEY);
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

	// Single-flight + latest-wins guards. Space and schema lists are fetched
	// from several components and on rapid space switches; without these,
	// duplicate requests pile up and a stale response can overwrite the
	// newest selection.
	const inflight = new Map<string, Promise<void>>();
	const requestSeq = new Map<string, number>();
	function requestOnce<T>(
		key: string,
		loader: () => Promise<T>,
		apply: (value: T) => void,
		onError: (error: unknown) => void,
	): Promise<void> {
		const existing = inflight.get(key);
		if (existing) return existing;
		const seq = (requestSeq.get(key) ?? 0) + 1;
		requestSeq.set(key, seq);
		const task = loader()
			.then((value) => {
				if (requestSeq.get(key) === seq) apply(value);
			})
			.catch((error) => {
				if (requestSeq.get(key) === seq) onError(error);
			})
			.finally(() => {
				if (inflight.get(key) === task) inflight.delete(key);
			});
		inflight.set(key, task);
		return task;
	}
	function invalidate(key: string): void {
		requestSeq.set(key, (requestSeq.get(key) ?? 0) + 1);
		inflight.delete(key);
	}

	const store = {
		subscribe,
		fetchSpaces: async () => {
			update((s) => ({ ...s, isLoadingSpaces: true }));
			return requestOnce(
				'spaces',
				() => schemaService.spaces.list(),
				(response) => {
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
				},
				(err) => {
					console.error('Fetch spaces error:', err);
					update((s) => ({ ...s, isLoadingSpaces: false }));
				},
			);
		},
		createSpace: async (params: CreateSpaceParams) => {
			const vidTypeStr =
				params.vidType === 'FIXED_STRING(32)' ? 'FIXED_STRING(32)' : 'INT64';
			const query = `CREATE SPACE IF NOT EXISTS ${params.name} (vid_type = ${vidTypeStr}, partition_num = ${params.partitionNum}, replica_factor = ${params.replicaFactor})`;
			await queryService.execute({ query });
			invalidate('spaces');
			await store.fetchSpaces();
		},
		deleteSpace: async (name: string) => {
			const query = `DROP SPACE IF EXISTS ${name}`;
			await queryService.execute({ query });
			invalidate('spaces');
			await store.fetchSpaces();
		},
		setCurrentSpace: (name: string | null) => {
			update((s) => ({ ...s, currentSpace: name }));
			persistCurrentSpace(name);
		},
		fetchTags: async (spaceName: string) => {
			update((s) => ({ ...s, isLoadingTags: true }));
			return requestOnce(
				`tags:${spaceName}`,
				() => schemaService.tags.list(spaceName),
				(response) => {
					const tags = Array.isArray(response)
						? response
						: (response as { data?: Tag[] }).data || [];
					update((s) => ({ ...s, tags, isLoadingTags: false }));
				},
				(err) => {
					console.error('Fetch tags error:', err);
					update((s) => ({ ...s, isLoadingTags: false }));
				},
			);
		},
		createTag: async (spaceName: string, params: CreateTagParams) => {
			await schemaService.tags.create(spaceName, params);
			invalidate(`tags:${spaceName}`);
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
				invalidate(`tags:${spaceName}`);
				await store.fetchTags(spaceName);
			}
		},
		deleteTag: async (spaceName: string, tagName: string) => {
			await schemaService.tags.delete(spaceName, tagName);
			invalidate(`tags:${spaceName}`);
			await store.fetchTags(spaceName);
		},
		fetchEdgeTypes: async (spaceName: string) => {
			update((s) => ({ ...s, isLoadingEdgeTypes: true }));
			return requestOnce(
				`edges:${spaceName}`,
				() => schemaService.edgeTypes.list(spaceName),
				(response) => {
					const edgeTypes = Array.isArray(response)
						? response
						: (response as { data?: EdgeType[] }).data || [];
					update((s) => ({ ...s, edgeTypes, isLoadingEdgeTypes: false }));
				},
				(err) => {
					console.error('Fetch edge types error:', err);
					update((s) => ({ ...s, isLoadingEdgeTypes: false }));
				},
			);
		},
		createEdgeType: async (spaceName: string, params: CreateEdgeTypeParams) => {
			await schemaService.edgeTypes.create(spaceName, params);
			invalidate(`edges:${spaceName}`);
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
				invalidate(`edges:${spaceName}`);
				await store.fetchEdgeTypes(spaceName);
			}
		},
		deleteEdgeType: async (spaceName: string, edgeName: string) => {
			await schemaService.edgeTypes.delete(spaceName, edgeName);
			invalidate(`edges:${spaceName}`);
			await store.fetchEdgeTypes(spaceName);
		},
		fetchIndexes: async (spaceName: string) => {
			update((s) => ({ ...s, isLoadingIndexes: true }));
			return requestOnce(
				`indexes:${spaceName}`,
				() => schemaService.indexes.list(spaceName),
				(response) => {
					const indexes = Array.isArray(response)
						? response
						: (response as { data?: IndexInfo[] }).data || [];
					update((s) => ({ ...s, indexes, isLoadingIndexes: false }));
				},
				(err) => {
					console.error('Fetch indexes error:', err);
					update((s) => ({ ...s, isLoadingIndexes: false }));
				},
			);
		},
		createIndex: async (spaceName: string, params: CreateIndexParams) => {
			await schemaService.indexes.create(spaceName, params);
			invalidate(`indexes:${spaceName}`);
			await store.fetchIndexes(spaceName);
		},
		deleteIndex: async (spaceName: string, indexName: string) => {
			await schemaService.indexes.delete(spaceName, indexName);
			invalidate(`indexes:${spaceName}`);
			await store.fetchIndexes(spaceName);
		},
		rebuildIndex: async (spaceName: string, indexName: string) => {
			await schemaService.indexes.rebuild(spaceName, indexName);
			invalidate(`indexes:${spaceName}`);
			await store.fetchIndexes(spaceName);
		},
	};
	return store;
}

export const schemaStore = createSchemaStore();

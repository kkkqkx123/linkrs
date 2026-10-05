import { call, client, unwrap } from '$lib/api/client';
import type {
	VertexDetail,
	WireEdgeDetail,
	NeighborInfo,
	NeighborParams,
} from '$types/graph';
import type { components } from '$lib/api/schema';

type Envelope = components['schemas']['ApiResponse_Value'];

// Normalize a scalar or structured id into a stable string. Mirrors the id
// coercion used when building graph elements so that nodes produced from
// different sources (query results, neighbor expansion) collapse onto the same
// identifier.
function stringifyId(value: unknown): string | null {
	if (value === null || value === undefined) return null;
	if (typeof value === 'string') return value;
	if (
		typeof value === 'number' ||
		typeof value === 'bigint' ||
		typeof value === 'boolean'
	) {
		return String(value);
	}
	if (typeof value === 'object') {
		try {
			return JSON.stringify(value);
		} catch {
			return String(value);
		}
	}
	return String(value);
}

function asRecord(value: unknown): Record<string, unknown> | null {
	if (typeof value === 'object' && value !== null && !Array.isArray(value)) {
		return value as Record<string, unknown>;
	}
	return null;
}

// Extract { vid, tag, properties } from a raw neighbor payload. The graph API
// nests each neighbor as { vertex: { vid, tag } } where tag is
// { name, properties }. Degenerate payloads may expose flat vid / tag fields,
// so both shapes are accepted.
function normalizeNeighbor(
	raw: unknown,
	fallbackDirection: NeighborParams['direction'],
): NeighborInfo | null {
	const wrapper = asRecord(raw);
	if (!wrapper) return null;

	const vertex = asRecord(wrapper.vertex) ?? wrapper;
	const vid = stringifyId(vertex.vid ?? wrapper.vid);
	if (vid === null) return null;

	let tag = 'unknown';
	let properties: Record<string, unknown> = {};
	const tagObj = asRecord(vertex.tag);
	if (tagObj && typeof tagObj.name === 'string') {
		tag = tagObj.name;
		properties = asRecord(tagObj.properties) ?? {};
	} else if (typeof vertex.tag === 'string') {
		tag = vertex.tag;
		properties =
			asRecord(vertex.properties) ?? asRecord(wrapper.properties) ?? {};
	} else if (asRecord(vertex.properties)) {
		properties = asRecord(vertex.properties) as Record<string, unknown>;
	}

	const direction =
		(wrapper.direction as NeighborInfo['direction']) ??
		fallbackDirection ??
		'BOTH';
	const edgeType = String(wrapper.edge_type ?? wrapper.edgeType ?? 'unknown');
	const rank =
		typeof wrapper.rank === 'number'
			? wrapper.rank
			: typeof wrapper.ranking === 'number'
				? wrapper.ranking
				: 0;

	return { vid, tag, properties, edge_type: edgeType, direction, rank };
}

function toVertexDetail(payload: unknown, vid: string | number): VertexDetail {
	const record = asRecord(payload);
	const vertex = (record && asRecord(record.vertex)) ?? record ?? {};
	const tags = asRecord(vertex.tags) ?? {};
	const normalized: Record<string, Record<string, unknown>> = {};
	for (const [key, value] of Object.entries(tags)) {
		normalized[key] = asRecord(value) ?? {};
	}
	return {
		vid: stringifyId(vertex.vid ?? vid) ?? String(vid),
		tags: normalized,
	};
}

export const graphService = {
	vertices: {
		get: async (vid: string | number, space: string): Promise<VertexDetail> => {
			const payload = unwrap(
				await call<Envelope>(
					client.GET('/api/v1/graph/vertices/{vid}', {
						params: { path: { vid: String(vid) }, query: { space } },
					}),
				),
			);
			return toVertexDetail(payload, vid);
		},
		getNeighbors: async (
			vid: string | number,
			space: string,
			params?: NeighborParams,
		): Promise<NeighborInfo[]> => {
			const payload = unwrap(
				await call<Envelope>(
					client.GET('/api/v1/graph/vertices/{vid}/neighbors', {
						params: {
							path: { vid: String(vid) },
							query: {
								space,
								direction: params?.direction,
								edge_type: params?.edge_type,
							},
						},
					}),
				),
			);
			const record = asRecord(payload);
			const raw =
				record && Array.isArray(record.neighbors) ? record.neighbors : [];
			return (raw as unknown[])
				.map((item) => normalizeNeighbor(item, params?.direction))
				.filter((item): item is NeighborInfo => item !== null);
		},
	},
	edges: {
		get: async (
			src: string | number,
			dst: string | number,
			space: string,
			edgeType: string,
			rank?: number,
		): Promise<WireEdgeDetail> => {
			const payload = unwrap(
				await call<Envelope>(
					client.GET('/api/v1/graph/edges', {
						params: {
							query: {
								space,
								src: String(src),
								dst: String(dst),
								edge_type: edgeType,
								rank: rank ?? 0,
							},
						},
					}),
				),
			);
			const record = asRecord(payload) ?? {};
			const edge = asRecord(record.edge) ?? record;
			return {
				src: edge.src ?? src,
				dst: edge.dst ?? dst,
				edge_type:
					typeof edge.edge_type === 'string' ? edge.edge_type : edgeType,
				rank: typeof edge.rank === 'number' ? edge.rank : (rank ?? 0),
				properties: asRecord(edge.properties) ?? {},
			} as WireEdgeDetail;
		},
	},
};

export default graphService;

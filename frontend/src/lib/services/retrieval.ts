import { operationsService } from '$services/operations';
import { vectorService } from '$services/vector';
import { fulltextService } from '$services/fulltext';

export type RetrievalProbe = 'reachable' | 'unreachable' | 'unknown' | 'disabled';

export interface VectorRetrievalStatus {
	enabled: boolean | null;
	engine: string | null;
	endpoint: string | null;
	transport: string | null;
	tls: boolean | null;
	indexCount: number | null;
	probe: RetrievalProbe;
}

export interface FulltextRetrievalStatus {
	enabled: boolean | null;
	engine: string | null;
	tokenizer: string | null;
	indexPath: string | null;
	inconsistentCount: number | null;
	probe: RetrievalProbe;
}

export interface RetrievalSnapshot {
	vector: VectorRetrievalStatus;
	fulltext: FulltextRetrievalStatus;
	embeddingConfigured: boolean | null;
	rerankConfigured: boolean | null;
}

function isRecord(value: unknown): value is Record<string, unknown> {
	return typeof value === 'object' && value !== null;
}

function asBool(value: unknown): boolean | null {
	return typeof value === 'boolean' ? value : null;
}

function asString(value: unknown): string | null {
	return typeof value === 'string' && value.length > 0 ? value : null;
}

function sectionOf(config: unknown, name: string): Record<string, unknown> {
	if (!isRecord(config)) return {};
	const section = config[name];
	return isRecord(section) ? section : {};
}

function countOf(payload: unknown): number | null {
	if (!isRecord(payload)) {
		if (Array.isArray(payload)) return payload.length;
		return null;
	}
	const direct = payload.count;
	if (typeof direct === 'number' && Number.isFinite(direct)) return direct;
	const indexes = payload.indexes;
	if (Array.isArray(indexes)) return indexes.length;
	return null;
}

function endpointOf(connection: unknown): {
	endpoint: string | null;
	transport: string | null;
	tls: boolean | null;
} {
	if (!isRecord(connection)) return { endpoint: null, transport: null, tls: null };
	const host = asString(connection.host);
	const port =
		typeof connection.port === 'number' ? connection.port : connection.http_port;
	const portText = typeof port === 'number' ? String(port) : null;
	return {
		endpoint: host && portText ? `${host}:${portText}` : host,
		transport: asString(connection.transport),
		tls: asBool(connection.use_tls),
	};
}

/**
 * Retrieval service health aggregation for the monitoring page.
 *
 * Combines the read-only config surface with the vector index listing and
 * the fulltext inconsistent listing. Every source is best-effort: admin-only
 * endpoints reject non-admin callers, in which case the affected fields stay
 * `unknown` instead of failing the whole snapshot. The vector listing doubles
 * as a reachability probe for the configured backend (local or external
 * Qdrant); a failed listing against an enabled backend surfaces as
 * `unreachable` so external outages become visible.
 */
export const retrievalService = {
	snapshot: async (): Promise<RetrievalSnapshot> => {
		const [configSettled, vectorSettled, fulltextSettled] =
			await Promise.allSettled([
				operationsService.config(),
				vectorService.list(),
				fulltextService.inconsistent(),
			]);

		const config =
			configSettled.status === 'fulfilled' ? configSettled.value : null;
		const vectorSection = sectionOf(config, 'vector');
		const fulltextSection = sectionOf(config, 'fulltext');

		const vectorEnabled = asBool(vectorSection.enabled);
		const vectorEngine = asString(vectorSection.engine);
		const { endpoint, transport, tls } = endpointOf(
			(vectorSection.qdrant as Record<string, unknown> | undefined)
				?.connection ?? vectorSection.connection,
		);
		const embeddingConfigured =
			vectorSection.embedding === undefined
				? null
				: vectorSection.embedding !== null;
		const rerankConfigured =
			vectorSection.rerank === undefined ? null : vectorSection.rerank !== null;

		let vectorProbe: RetrievalProbe;
		let vectorIndexCount: number | null = null;
		if (vectorEnabled === false) {
			vectorProbe = 'disabled';
		} else if (vectorSettled.status === 'fulfilled') {
			vectorProbe = 'reachable';
			vectorIndexCount = countOf(vectorSettled.value);
		} else if (vectorEnabled === true) {
			vectorProbe = 'unreachable';
		} else {
			vectorProbe = 'unknown';
		}

		const fulltextEnabled = asBool(fulltextSection.enabled);
		const fulltextEngine = asString(fulltextSection.default_engine);
		const tantivy = isRecord(fulltextSection.tantivy)
			? (fulltextSection.tantivy as Record<string, unknown>)
			: {};
		let fulltextProbe: RetrievalProbe;
		let inconsistentCount: number | null = null;
		if (fulltextEnabled === false) {
			fulltextProbe = 'disabled';
		} else if (fulltextSettled.status === 'fulfilled') {
			fulltextProbe = 'reachable';
			inconsistentCount = countOf(fulltextSettled.value);
		} else if (fulltextEnabled === true) {
			fulltextProbe = 'unreachable';
		} else {
			fulltextProbe = 'unknown';
		}

		return {
			vector: {
				enabled: vectorEnabled,
				engine: vectorEngine,
				endpoint,
				transport,
				tls,
				indexCount: vectorIndexCount,
				probe: vectorProbe,
			},
			fulltext: {
				enabled: fulltextEnabled,
				engine: fulltextEngine,
				tokenizer: asString(tantivy.tokenizer),
				indexPath: asString(fulltextSection.index_path),
				inconsistentCount,
				probe: fulltextProbe,
			},
			embeddingConfigured,
			rerankConfigured,
		};
	},

};

export default retrievalService;

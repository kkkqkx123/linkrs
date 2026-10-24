import { call, client } from '$lib/api/client';
import { getApiBaseUrl, getSessionHeaders } from '$utils/http';

export interface ImportParams {
	space: string;
	format: string;
	target_type: string;
	target_name: string;
	batch_size?: number;
	file: File;
}

/**
 * Data transfer: multipart import plus the synchronous import-status
 * contract and the re-executing server export (`GET /v1/export`).
 * Multipart upload uses raw fetch because openapi-fetch cannot model
 * the file part; polling uses the typed client.
 */
export const transferService = {
	importFile: async (params: ImportParams): Promise<unknown> => {
		const form = new FormData();
		form.append('space', params.space);
		form.append('format', params.format);
		form.append('target_type', params.target_type);
		form.append('target_name', params.target_name);
		if (params.batch_size !== undefined) {
			form.append('batch_size', String(params.batch_size));
		}
		form.append('file', params.file);
		const headers = getSessionHeaders();
		const sessionHeader = (headers as Record<string, string>)['X-Session-ID'];
		const response = await fetch(`${getApiBaseUrl()}/v1/import`, {
			method: 'POST',
			headers: sessionHeader ? { 'X-Session-ID': sessionHeader } : {},
			body: form,
		});
		if (!response.ok) {
			const detail = await response.text().catch(() => '');
			throw new Error(
				`Import failed with status ${response.status}${detail ? `: ${detail.slice(0, 300)}` : ''}`,
			);
		}
		return (await response.json().catch(() => null)) as unknown;
	},

	importStatus: async (id: string): Promise<unknown> =>
		call(
			client.GET('/v1/import/{id}', {
				params: { path: { id } },
			}),
		),

	exportQuery: async (
		query: string,
		format: 'csv' | 'jsonl',
		sessionId: number,
	): Promise<Blob> => {
		const params = new URLSearchParams({
			session_id: String(sessionId),
			format,
			query,
		});
		const response = await fetch(
			`${getApiBaseUrl()}/v1/export?${params.toString()}`,
			{ headers: { ...getSessionHeaders() } },
		);
		if (!response.ok) {
			const detail = await response.text().catch(() => '');
			throw new Error(
				`Export failed with status ${response.status}${detail ? `: ${detail.slice(0, 300)}` : ''}`,
			);
		}
		return await response.blob();
	},
};

export default transferService;

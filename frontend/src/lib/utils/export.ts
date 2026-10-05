import type { QueryResult } from '$types/query';
import {
	getApiBaseUrl,
	getSessionHeaders,
	resolveSessionId,
} from '$utils/http';
import { t } from '$i18n';

function saveBlob(blob: Blob, filename: string): void {
	const link = document.createElement('a');
	const url = URL.createObjectURL(blob);
	link.setAttribute('href', url);
	link.setAttribute('download', filename);
	link.style.visibility = 'hidden';
	document.body.appendChild(link);
	link.click();
	document.body.removeChild(link);
	URL.revokeObjectURL(url);
}

function fileNameFromDisposition(
	header: string | null,
	fallback: string,
): string {
	if (!header) return fallback;
	const match = /filename\*?=(?:UTF-8'')?"?([^";]+)"?/i.exec(header);
	if (!match) return fallback;
	try {
		return decodeURIComponent(match[1].trim());
	} catch {
		return match[1].trim() || fallback;
	}
}

export const exportToCSV = (result: QueryResult, filename?: string): void => {
	if (!result || !result.columns || result.columns.length === 0) return;
	const { columns, rows } = result;
	const formatValue = (value: unknown): string => {
		if (value === null || value === undefined) return '';
		if (typeof value === 'object') return JSON.stringify(value);
		return String(value);
	};
	const escapeField = (field: unknown): string => {
		if (field === null || field === undefined) return '';
		const str = String(field);
		if (
			str.includes(',') ||
			str.includes('"') ||
			str.includes('\n') ||
			str.includes('\r')
		) {
			return `"${str.replace(/"/g, '""')}"`;
		}
		return str;
	};
	const csvRows: string[] = [columns.map(escapeField).join(',')];
	rows.forEach((row) =>
		csvRows.push(
			columns.map((col) => escapeField(formatValue(row[col]))).join(','),
		),
	);
	const csvContent = csvRows.join('\n');
	const blob = new Blob([csvContent], { type: 'text/csv;charset=utf-8;' });
	saveBlob(blob, filename || `query_result_${Date.now()}.csv`);
};

export const exportToJSON = (result: QueryResult, filename?: string): void => {
	if (!result) return;
	const jsonContent = JSON.stringify(result, null, 2);
	const blob = new Blob([jsonContent], {
		type: 'application/json;charset=utf-8;',
	});
	saveBlob(blob, filename || `query_result_${Date.now()}.json`);
};

/**
 * Export a stream result through the server: the server re-executes the
 * statement and streams CSV/JSONL chunk-at-a-time, but the browser still
 * buffers the response into a blob before saving. A plain anchor download
 * cannot send the session auth header, so scripted fetch is kept and very
 * large exports reside in memory until the save completes. A failed
 * download leaves no partial file.
 */
export const exportStreamViaServer = async (
	query: string,
	format: 'csv' | 'jsonl',
): Promise<void> => {
	const sessionId = resolveSessionId();
	if (sessionId === undefined) {
		throw new Error(t('notification.missingSessionForExport'));
	}
	const params = new URLSearchParams({
		session_id: String(sessionId),
		format,
		query,
	});
	const response = await fetch(
		`${getApiBaseUrl()}/v1/export?${params.toString()}`,
		{
			method: 'GET',
			headers: { ...getSessionHeaders() },
		},
	);
	if (!response.ok) {
		const detail = await response.text().catch(() => '');
		throw new Error(
			`Server export failed with status ${response.status}${detail ? `: ${detail.slice(0, 200)}` : ''}`,
		);
	}
	const blob = await response.blob();
	const filename = fileNameFromDisposition(
		response.headers.get('content-disposition'),
		`query_result_${Date.now()}.${format}`,
	);
	saveBlob(blob, filename);
};

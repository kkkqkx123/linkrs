import { getApiBaseUrl, getSessionHeaders } from '$utils/http';
import {
	MigrationSseParser,
	type MigrationProgressEvent,
} from '$utils/migrationEvents';

export type { MigrationProgressEvent };

export interface MigrationStreamOutcome {
	cancelled: boolean;
	finished: boolean;
	eventsReceived: number;
	streamError: string | null;
}

/**
 * Streaming exception: the SSE endpoint streams `text/event-stream` frames,
 * which OpenAPI cannot model, so it uses raw fetch with the session helpers
 * and is not covered by codegen (same rationale as the query stream).
 * Frames are decoded into typed `MigrationProgressEvent` values.
 */
export async function streamMigrationProgress(
	space: string,
	label: string,
	options: {
		isEdge?: boolean;
		signal?: AbortSignal;
		onEvent?: (event: MigrationProgressEvent) => void;
	} = {},
): Promise<MigrationStreamOutcome> {
	const outcome: MigrationStreamOutcome = {
		cancelled: false,
		finished: false,
		eventsReceived: 0,
		streamError: null,
	};
	const query =
		options.isEdge === undefined ? '' : `?is_edge=${options.isEdge}`;
	let response: Response;
	try {
		response = await fetch(
			`${getApiBaseUrl()}/v1/migration/stream/${encodeURIComponent(space)}/${encodeURIComponent(label)}${query}`,
			{
				headers: { ...getSessionHeaders(), Accept: 'text/event-stream' },
				signal: options.signal,
			},
		);
	} catch (error) {
		if (error instanceof DOMException && error.name === 'AbortError') {
			outcome.cancelled = true;
			return outcome;
		}
		outcome.streamError =
			error instanceof Error ? error.message : 'Failed to open migration stream';
		return outcome;
	}
	if (!response.ok || !response.body) {
		outcome.streamError = `Migration stream failed with status ${response.status}`;
		return outcome;
	}
	const parser = new MigrationSseParser();
	const reader = response.body.getReader();
	const decoder = new TextDecoder();
	try {
		for (;;) {
			const { done, value } = await reader.read();
			if (done) break;
			for (const event of parser.feed(decoder.decode(value, { stream: true }))) {
				outcome.eventsReceived += 1;
				options.onEvent?.(event);
			}
			if (parser.finished) break;
			if (options.signal?.aborted) break;
		}
		for (const event of parser.feed(decoder.decode())) {
			outcome.eventsReceived += 1;
			options.onEvent?.(event);
		}
	} catch (error) {
		if (
			(error instanceof DOMException && error.name === 'AbortError') ||
			options.signal?.aborted
		) {
			outcome.cancelled = true;
			return outcome;
		}
		outcome.streamError =
			error instanceof Error ? error.message : 'Migration stream interrupted';
		return outcome;
	} finally {
		reader.releaseLock();
	}
	outcome.finished = parser.finished;
	return outcome;
}

/**
 * Minimal server-sent events frame parser.
 *
 * Splits a raw byte stream into `event:`/`data:` frames per the SSE wire
 * format. Comment lines (heartbeats such as `: keepalive`) are ignored.
 * Used by monitoring-style streams that consume arbitrary named events.
 */

export interface SseFrame {
	/** Event name from the `event:` field; empty string when unset. */
	event: string;
	/** Joined payload of all `data:` lines. */
	data: string;
}

const FRAME_SEPARATOR = /\r?\n\r?\n/;

/** Incremental reader that buffers partial chunks until frames complete. */
export class SseFrameReader {
	private buffer = '';

	/** Feed a decoded text chunk and return all frames that became complete. */
	feed(chunk: string): SseFrame[] {
		this.buffer += chunk;
		const frames: SseFrame[] = [];
		let boundary = this.buffer.search(FRAME_SEPARATOR);
		while (boundary !== -1) {
			const raw = this.buffer.slice(0, boundary);
			this.buffer = this.buffer.slice(boundary).replace(/^\r?\n\r?\n/, '');
			const frame = parseFrame(raw);
			if (frame) frames.push(frame);
			boundary = this.buffer.search(FRAME_SEPARATOR);
		}
		return frames;
	}

	/** Flush any trailing frame that lacks a final separator. */
	flush(): SseFrame[] {
		const rest = this.buffer;
		this.buffer = '';
		const frame = parseFrame(rest);
		return frame ? [frame] : [];
	}
}

/** Parse one raw frame block into an event, or null for comments/empty blocks. */
export function parseFrame(raw: string): SseFrame | null {
	let event = '';
	const dataLines: string[] = [];
	for (const line of raw.split(/\r?\n/)) {
		if (!line || line.startsWith(':')) continue;
		const separator = line.indexOf(':');
		const field = separator === -1 ? line : line.slice(0, separator);
		let value = separator === -1 ? '' : line.slice(separator + 1);
		if (value.startsWith(' ')) value = value.slice(1);
		if (field === 'event') event = value;
		else if (field === 'data') dataLines.push(value);
	}
	if (event === '' && dataLines.length === 0) return null;
	return { event, data: dataLines.join('\n') };
}

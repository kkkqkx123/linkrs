import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { parseFrame, SseFrameReader } from './sseFrames.ts';

function feedAll(reader: SseFrameReader, chunks: string[]) {
	return chunks.flatMap((chunk) => reader.feed(chunk));
}

describe('parseFrame', () => {
	it('parses event and data fields', () => {
		assert.deepEqual(parseFrame('event: snapshot\ndata: {"ok":true}'), {
			event: 'snapshot',
			data: '{"ok":true}',
		});
	});

	it('joins multiple data lines with newlines', () => {
		assert.deepEqual(parseFrame('event: log\ndata: one\ndata: two'), {
			event: 'log',
			data: 'one\ntwo',
		});
	});

	it('ignores comments and blank blocks', () => {
		assert.equal(parseFrame(': keepalive'), null);
		assert.equal(parseFrame(''), null);
	});

	it('supports fields without a value separator', () => {
		assert.deepEqual(parseFrame('data'), { event: '', data: '' });
	});
});

describe('SseFrameReader', () => {
	it('buffers partial chunks until a full frame arrives', () => {
		const reader = new SseFrameReader();
		assert.equal(reader.feed('event: snapshot\nda').length, 0);
		const frames = reader.feed('ta: {"n":1}\n\n');
		assert.deepEqual(frames, [{ event: 'snapshot', data: '{"n":1}' }]);
	});

	it('returns multiple frames from one chunk', () => {
		const reader = new SseFrameReader();
		const frames = feedAll(reader, [
			'event: a\ndata: 1\n\nevent: b\ndata: 2\n\n: keepalive\n\n',
		]);
		assert.deepEqual(frames, [
			{ event: 'a', data: '1' },
			{ event: 'b', data: '2' },
		]);
	});

	it('handles CRLF separators', () => {
		const reader = new SseFrameReader();
		const frames = reader.feed('event: ping\r\ndata: hi\r\n\r\n');
		assert.deepEqual(frames, [{ event: 'ping', data: 'hi' }]);
	});

	it('flushes a trailing frame without separator', () => {
		const reader = new SseFrameReader();
		assert.equal(reader.feed('event: tail\ndata: end').length, 0);
		assert.deepEqual(reader.flush(), [{ event: 'tail', data: 'end' }]);
		assert.deepEqual(reader.flush(), []);
	});

	it('keeps buffering across single-character chunks', () => {
		const reader = new SseFrameReader();
		const received = feedAll(
			reader,
			'event: snapshot\ndata: {"i":9}\n\n'.split(''),
		);
		assert.deepEqual(received, [{ event: 'snapshot', data: '{"i":9}' }]);
	});
});

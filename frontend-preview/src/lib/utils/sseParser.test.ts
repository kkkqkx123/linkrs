import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { SseParser, parseJsonTolerant } from './sseParser.ts';

function feedAll(parser: SseParser, chunks: string[]) {
	return chunks.flatMap((chunk) => parser.feed(chunk));
}

describe('SseParser', () => {
	it('reassembles a frame split across network chunks', () => {
		const parser = new SseParser();
		const frame =
			'event: schema\ndata: {"columns": ["a", "b"], "column_count": 2, "stmt": 0}\n\n';
		const cut = Math.floor(frame.length / 2);
		const first = parser.feed(frame.slice(0, cut));
		assert.equal(first.length, 0);
		const second = parser.feed(frame.slice(cut));
		assert.equal(second.length, 1);
		assert.deepEqual(second[0], {
			kind: 'schema',
			columns: ['a', 'b'],
			stmt: 0,
		});
	});

	it('discards comment frames used for connection keepalive', () => {
		const parser = new SseParser();
		const events = feedAll(parser, [
			': keepalive\n\n',
			'event: done\ndata: {}\n\n',
		]);
		assert.equal(events.length, 1);
		assert.deepEqual(events[0], { kind: 'done' });
		assert.equal(parser.doneReceived, true);
	});

	it('joins multi-line data fields before parsing', () => {
		const parser = new SseParser();
		const events = parser.feed(
			'data: {"row": {"a":\ndata: 1}, "index": 4, "stmt": 0}\n\n',
		);
		assert.equal(events.length, 1);
		assert.deepEqual(events[0], {
			kind: 'row',
			row: { a: 1 },
			index: 4,
			stmt: 0,
		});
	});

	it('parses row and metadata events in arrival order', () => {
		const parser = new SseParser();
		const events = feedAll(parser, [
			'event: schema\ndata: {"columns": ["n"], "column_count": 1, "stmt": 0}\n\n',
			'data: {"row": {"n": 1}, "index": 0, "stmt": 0}\n\ndata: {"row": {"n": 2}, "index": 1, "stmt": 0}\n\n',
			'event: metadata\ndata: {"rows_returned": 2, "execution_time_ms": 9, "columns": [], "stmt": 0}\n\n',
			'event: done\ndata: {}\n\n',
		]);
		assert.deepEqual(events, [
			{ kind: 'schema', columns: ['n'], stmt: 0 },
			{ kind: 'row', row: { n: 1 }, index: 0, stmt: 0 },
			{ kind: 'row', row: { n: 2 }, index: 1, stmt: 0 },
			{ kind: 'metadata', rowsReturned: 2, executionTimeMs: 9, stmt: 0 },
			{ kind: 'done' },
		]);
	});

	it('surfaces error events while keeping rows already received', () => {
		const parser = new SseParser();
		const events = feedAll(parser, [
			'data: {"row": {"n": 1}, "index": 0, "stmt": 0}\n\n',
			'event: error\ndata: {"error": true, "message": "boom", "code": "QUERY_ERROR", "stmt": 0}\n\n',
			'event: done\ndata: {}\n\n',
		]);
		assert.equal(events.length, 3);
		assert.equal(events[0].kind, 'row');
		assert.deepEqual(events[1], {
			kind: 'error',
			code: 'QUERY_ERROR',
			message: 'boom',
			stmt: 0,
		});
		assert.deepEqual(events[2], { kind: 'done' });
	});

	it('reports a missing end marker through the done flag', () => {
		const parser = new SseParser();
		const events = feedAll(parser, [
			'event: schema\ndata: {"columns": ["n"], "column_count": 1, "stmt": 0}\n\n',
			'data: {"row": {"n": 1}, "index": 0, "stmt": 0}\n\n',
		]);
		assert.equal(events.length, 2);
		assert.equal(parser.doneReceived, false);
	});

	it('parses large integers with the bigint-tolerant decoder', () => {
		const parsed = parseJsonTolerant('{"id": 9223372036854775807}') as Record<
			string,
			unknown
		>;
		assert.equal(String(parsed['id']), '9223372036854775807');
	});

	it('parses batch statement boundaries around per-statement events', () => {
		const parser = new SseParser();
		const events = feedAll(parser, [
			'event: statement_begin\ndata: {"index": 0, "query": "RETURN 1"}\n\n',
			'event: schema\ndata: {"columns": ["n"], "column_count": 1, "stmt": 0}\n\n',
			'data: {"row": {"n": 1}, "index": 0, "stmt": 0}\n\n',
			'event: statement_end\ndata: {"index": 0, "success": true, "rows_returned": 1, "execution_time_ms": 3}\n\n',
			'event: statement_begin\ndata: {"index": 1, "query": "RETURN 2"}\n\n',
			'event: error\ndata: {"error": true, "message": "boom", "code": "QUERY_ERROR", "stmt": 1}\n\n',
			'event: statement_end\ndata: {"index": 1, "success": false, "rows_returned": 0, "execution_time_ms": 1, "code": "QUERY_ERROR", "message": "boom"}\n\n',
			'event: done\ndata: {}\n\n',
		]);
		assert.deepEqual(events, [
			{ kind: 'statement_begin', index: 0, query: 'RETURN 1' },
			{ kind: 'schema', columns: ['n'], stmt: 0 },
			{ kind: 'row', row: { n: 1 }, index: 0, stmt: 0 },
			{
				kind: 'statement_end',
				index: 0,
				success: true,
				rowsReturned: 1,
				executionTimeMs: 3,
				code: null,
				message: null,
			},
			{ kind: 'statement_begin', index: 1, query: 'RETURN 2' },
			{ kind: 'error', code: 'QUERY_ERROR', message: 'boom', stmt: 1 },
			{
				kind: 'statement_end',
				index: 1,
				success: false,
				rowsReturned: 0,
				executionTimeMs: 1,
				code: 'QUERY_ERROR',
				message: 'boom',
			},
			{ kind: 'done' },
		]);
	});

	it('routes interleaved batch rows by statement number', () => {
		const parser = new SseParser();
		const events = feedAll(parser, [
			'event: statement_begin\ndata: {"index": 0, "query": "RETURN 1"}\n\n',
			'event: statement_begin\ndata: {"index": 1, "query": "RETURN 2"}\n\n',
			'data: {"row": {"n": 2}, "index": 0, "stmt": 1}\n\n',
			'data: {"row": {"n": 1}, "index": 0, "stmt": 0}\n\n',
			'event: done\ndata: {}\n\n',
		]);
		assert.deepEqual(events, [
			{ kind: 'statement_begin', index: 0, query: 'RETURN 1' },
			{ kind: 'statement_begin', index: 1, query: 'RETURN 2' },
			{ kind: 'row', row: { n: 2 }, index: 0, stmt: 1 },
			{ kind: 'row', row: { n: 1 }, index: 0, stmt: 0 },
			{ kind: 'done' },
		]);
	});

	it('drops boundary events with a missing statement index', () => {
		const parser = new SseParser();
		const events = feedAll(parser, [
			'event: statement_begin\ndata: {"query": "RETURN 1"}\n\n',
			'event: statement_end\ndata: {"success": true}\n\n',
			'event: done\ndata: {}\n\n',
		]);
		assert.deepEqual(events, [{ kind: 'done' }]);
	});
});

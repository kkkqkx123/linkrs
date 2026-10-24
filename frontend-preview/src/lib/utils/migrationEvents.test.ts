import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import {
	MigrationSseParser,
	isTerminalMigrationEvent,
	parseMigrationEvent,
} from './migrationEvents.ts';

describe('parseMigrationEvent', () => {
	it('parses the started frame', () => {
		assert.deepEqual(
			parseMigrationEvent('started', {
				plan_hash: 'abc',
				space: 's',
				label: 'User',
				is_edge: false,
			}),
			{
				kind: 'started',
				planHash: 'abc',
				space: 's',
				label: 'User',
				isEdge: false,
			},
		);
	});

	it('parses step lifecycle frames', () => {
		assert.deepEqual(parseMigrationEvent('step_started', { step_idx: 2 }), {
			kind: 'step_started',
			stepIdx: 2,
		});
		assert.deepEqual(
			parseMigrationEvent('step_completed', { step_idx: 2, rows: 40 }),
			{ kind: 'step_completed', stepIdx: 2, rows: 40 },
		);
	});

	it('parses terminal frames and reports them as terminal', () => {
		const completed = parseMigrationEvent('completed', {
			success: true,
			steps_completed: 3,
			rows_migrated: 100,
			errors: [],
		});
		assert.deepEqual(completed, {
			kind: 'completed',
			success: true,
			stepsCompleted: 3,
			rowsMigrated: 100,
			errors: [],
		});
		assert.equal(isTerminalMigrationEvent(completed ?? { kind: 'failed', error: '' }), true);
		assert.deepEqual(parseMigrationEvent('failed', { error: 'boom' }), {
			kind: 'failed',
			error: 'boom',
		});
		assert.deepEqual(
			parseMigrationEvent('rolled_back', {
				success: true,
				steps_completed: 1,
				rows_migrated: 7,
			}),
			{
				kind: 'rolled_back',
				success: true,
				stepsCompleted: 1,
				rowsMigrated: 7,
			},
		);
	});

	it('rejects unknown names and malformed payloads', () => {
		assert.equal(parseMigrationEvent('nope', {}), null);
		assert.equal(parseMigrationEvent('step_started', { step_idx: -1 }), null);
		assert.equal(parseMigrationEvent('step_started', {}), null);
		assert.deepEqual(parseMigrationEvent('started', null), {
			kind: 'started',
			planHash: '',
			space: '',
			label: '',
			isEdge: false,
		});
	});
});

describe('MigrationSseParser', () => {
	it('reassembles a frame split across chunks', () => {
		const parser = new MigrationSseParser();
		const frame =
			'event: step_completed\ndata: {"type": "step_completed", "step_idx": 1, "rows": 9}\n\n';
		const cut = Math.floor(frame.length / 2);
		assert.deepEqual(parser.feed(frame.slice(0, cut)), []);
		assert.deepEqual(parser.feed(frame.slice(cut)), [
			{ kind: 'step_completed', stepIdx: 1, rows: 9 },
		]);
	});

	it('skips keepalive comments and stops at terminal events', () => {
		const parser = new MigrationSseParser();
		const events = parser.feed(
			': keepalive\n\nevent: started\ndata: {"plan_hash": "h", "space": "s", "label": "L"}\n\nevent: completed\ndata: {"success": true, "steps_completed": 1, "rows_migrated": 5, "errors": []}\n\n',
		);
		assert.equal(events.length, 2);
		assert.equal(events[0].kind, 'started');
		assert.equal(events[1].kind, 'completed');
		assert.equal(parser.finished, true);
		assert.deepEqual(parser.feed('event: failed\ndata: {}\n\n'), []);
	});

	it('drops malformed frames without throwing', () => {
		const parser = new MigrationSseParser();
		assert.deepEqual(parser.feed('event: step_started\ndata: not-json\n\n'), []);
		assert.equal(parser.finished, false);
	});
});

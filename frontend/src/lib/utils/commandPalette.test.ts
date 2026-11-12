import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { filterCommands, type PaletteCommand } from './commandPalette.ts';

function command(id: string, label: string, keywords?: string): PaletteCommand {
	return {
		id,
		label: () => label,
		keywords: keywords ? () => keywords : undefined,
		run: () => {},
	};
}

describe('filterCommands', () => {
	const commands = [
		command('/console', 'Query Console', '/console'),
		command('/data-browser', 'Data Browser', '/data-browser'),
		command('/monitoring', 'Monitoring', '/monitoring'),
	];

	it('returns every command for an empty query', () => {
		assert.equal(filterCommands(commands, '').length, 3);
		assert.equal(filterCommands(commands, '   ').length, 3);
	});

	it('matches label substrings case-insensitively', () => {
		const hits = filterCommands(commands, 'browser');
		assert.deepEqual(
			hits.map((c) => c.id),
			['/data-browser'],
		);
		assert.equal(filterCommands(commands, 'CONSOLE').length, 1);
	});

	it('matches word initials so "db" hits "Data Browser"', () => {
		const hits = filterCommands(commands, 'db');
		assert.deepEqual(
			hits.map((c) => c.id),
			['/data-browser'],
		);
	});

	it('matches keyword substrings (route paths)', () => {
		const hits = filterCommands(commands, '/data');
		assert.equal(hits.length, 1);
		assert.equal(hits[0].id, '/data-browser');
	});

	it('returns nothing for a query that hits no command', () => {
		assert.deepEqual(filterCommands(commands, 'zzz'), []);
	});

	it('preserves the original order among matches', () => {
		const ordered = [
			command('a', 'Alpha Action'),
			command('b', 'Beta Action'),
			command('c', 'Gamma Action'),
		];
		assert.deepEqual(
			filterCommands(ordered, 'action').map((c) => c.id),
			['a', 'b', 'c'],
		);
	});
});

import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { can, highestUiRole, resolveUiRole, toUiRole } from './roles.ts';

describe('toUiRole', () => {
	it('maps backend roles to UI roles', () => {
		assert.equal(toUiRole('GOD'), 'admin');
		assert.equal(toUiRole('ADMIN'), 'admin');
		assert.equal(toUiRole('DBA'), 'operator');
		assert.equal(toUiRole('USER'), 'operator');
		assert.equal(toUiRole('GUEST'), 'viewer');
	});

	it('keeps legacy missing roles as null', () => {
		assert.equal(toUiRole(null), null);
		assert.equal(toUiRole(undefined), null);
		assert.equal(toUiRole(''), null);
	});

	it('downgrades unknown values to viewer', () => {
		assert.equal(toUiRole('SOMETHING'), 'viewer');
	});
});

describe('highestUiRole', () => {
	it('picks the strongest role', () => {
		assert.equal(highestUiRole(['GUEST', 'USER']), 'operator');
		assert.equal(highestUiRole(['GUEST', 'ADMIN']), 'admin');
		assert.equal(highestUiRole([]), null);
	});
});

describe('resolveUiRole', () => {
	it('reads role fields from payloads', () => {
		assert.equal(resolveUiRole({ role: 'GUEST' }), 'viewer');
		assert.equal(resolveUiRole({ display_role: 'DBA' }), 'operator');
		assert.equal(resolveUiRole({ roles: ['GUEST', 'ADMIN'] }), 'admin');
	});

	it('preserves legacy access for single-user backends', () => {
		assert.equal(resolveUiRole({}, 'root'), 'admin');
		assert.equal(resolveUiRole({}, 'alice'), 'admin');
		assert.equal(resolveUiRole({}), null);
	});
});

describe('can', () => {
	it('grants full access without a role', () => {
		assert.equal(can('manageUsers', null), true);
		assert.equal(can('write', null), true);
	});

	it('restricts viewer and operator as designed', () => {
		assert.equal(can('write', 'viewer'), false);
		assert.equal(can('manageUsers', 'operator'), false);
		assert.equal(can('manageUsers', 'admin'), true);
		assert.equal(can('dropSpace', 'operator'), false);
		assert.equal(can('alterSchema', 'operator'), true);
	});
});

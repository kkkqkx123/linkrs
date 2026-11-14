import { describe, it } from 'node:test';
import assert from 'node:assert/strict';
import { appendAuditLog, clearAuditLog, loadAuditLog } from './auditLog.ts';

// localStorage is unavailable under node:test, so the helpers must degrade
// to in-memory-safe behavior: loads fall back to empty, appends never throw.
describe('auditLog', () => {
	it('loads empty and clears without storage', () => {
		clearAuditLog();
		assert.deepEqual(loadAuditLog(), []);
	});

	it('appends without throwing when storage is unavailable', () => {
		clearAuditLog();
		const next = appendAuditLog('config', 'config a.b updated');
		assert.ok(Array.isArray(next));
	});
});

import { storage } from './storage.ts';

export interface AuditEntry {
	at: number;
	kind: string;
	summary: string;
}

const STORAGE_KEY = 'ops.audit';
const MAX_ENTRIES = 100;

export function loadAuditLog(): AuditEntry[] {
	const entries = storage.get<AuditEntry[]>(STORAGE_KEY, []);
	return Array.isArray(entries) ? entries : [];
}

export function appendAuditLog(kind: string, summary: string): AuditEntry[] {
	const entry: AuditEntry = { at: Date.now(), kind, summary };
	const next = [...loadAuditLog(), entry].slice(-MAX_ENTRIES);
	storage.set(STORAGE_KEY, next);
	return next;
}

export function clearAuditLog(): void {
	storage.remove(STORAGE_KEY);
}

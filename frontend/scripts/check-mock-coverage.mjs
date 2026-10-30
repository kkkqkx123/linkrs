import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';

const root = process.cwd();
const spec = JSON.parse(readFileSync(join(root, 'openapi.json'), 'utf8'));

const openapiKeys = new Set();
for (const [path, ops] of Object.entries(spec.paths ?? {})) {
	for (const method of Object.keys(ops)) {
		if (['get', 'post', 'put', 'delete', 'patch'].includes(method)) {
			openapiKeys.add(`${method.toUpperCase()} ${path}`);
		}
	}
}

const registered = new Set();
const handlerDir = join(root, 'src/lib/mock/handlers');
for (const file of readdirSync(handlerDir)) {
	if (!file.endsWith('.ts')) continue;
	const text = readFileSync(join(handlerDir, file), 'utf8');
	for (const match of text.matchAll(/'(GET|POST|PUT|DELETE|PATCH)\s+([^']+)'/g)) {
		registered.add(`${match[1]} ${match[2]}`);
	}
}

// Endpoints that are intentionally mocked inside services (non-JSON transports).
const serviceMocked = new Set([
	'POST /v1/query/stream',
	'GET /v1/migration/stream/{space}/{label}',
	'POST /v1/import',
	'GET /v1/export',
]);

const used = new Set();
function walk(dir) {
	for (const entry of readdirSync(dir, { withFileTypes: true })) {
		const full = join(dir, entry.name);
		if (entry.isDirectory()) walk(full);
		else if (entry.name.endsWith('.ts') || entry.name.endsWith('.svelte')) {
			const text = readFileSync(full, 'utf8');
			for (const match of text.matchAll(/client\.(GET|POST|PUT|DELETE|PATCH)\(\s*'([^']+)'/g)) {
				used.add(`${match[1]} ${match[2]}`);
			}
		}
	}
}
walk(join(root, 'src'));

const usedMissing = [...used].filter((k) => !registered.has(k) && !serviceMocked.has(k)).sort();
const registeredUnknown = [...registered].filter((k) => !openapiKeys.has(k)).sort();

let failed = false;
if (usedMissing.length) {
	failed = true;
	console.error('Mock coverage: used but unregistered endpoints:');
	for (const key of usedMissing) console.error(`  - ${key}`);
}
if (registeredUnknown.length) {
	failed = true;
	console.error('Mock coverage: registered but not in OpenAPI:');
	for (const key of registeredUnknown) console.error(`  - ${key}`);
}

if (failed) process.exit(1);
console.log(`Mock coverage OK: ${registered.size} registered, ${used.size} used.`);

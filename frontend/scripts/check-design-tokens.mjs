#!/usr/bin/env node
/**
 * Keeps the shared design token catalogue in sync with the web stylesheet.
 *
 * `design/tokens.json` is the machine-readable source consumed by the desktop
 * client. The web studio keeps its variables in `src/app.css` because Tailwind
 * reads them at build time, so this check fails when either side drifts.
 */
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const frontend = fileURLToPath(new URL('..', import.meta.url));
const css = readFileSync(join(frontend, 'src/app.css'), 'utf8');
const tokens = JSON.parse(
	readFileSync(join(frontend, '..', 'design', 'tokens.json'), 'utf8'),
);

function declarations(text) {
	const result = {};
	const pattern = /(--[\w-]+)\s*:\s*([^;{}]+);/g;
	for (const match of text.matchAll(pattern)) {
		result[match[1]] = match[2].trim().replace(/\s+/g, ' ');
	}
	return result;
}

function block(pattern) {
	const match = css.match(pattern);
	if (!match) throw new Error(`cannot locate CSS block for ${pattern}`);
	return declarations(match[1]);
}

function merged(pattern) {
	const result = {};
	const global = new RegExp(pattern.source, pattern.flags.includes('g') ? pattern.flags : `${pattern.flags}g`);
	for (const match of css.matchAll(global)) {
		Object.assign(result, declarations(match[1]));
	}
	if (Object.keys(result).length === 0)
		throw new Error(`cannot locate CSS block for ${pattern}`);
	return result;
}

const actual = {
	theme: merged(/@theme(?:\s+inline)?\s*\{([\s\S]*?)\}/),
	root: block(/:root\s*\{([\s\S]*?)\}/),
	dense: block(/:root\[data-density='dense'\]\s*\{([\s\S]*?)\}/),
	dark: block(/\.dark\s*\{([\s\S]*?)\}/),
};

const failures = [];
for (const section of ['theme', 'root', 'dense', 'dark']) {
	const expected = tokens[section];
	if (!expected) {
		failures.push(`design/tokens.json is missing the "${section}" section`);
		continue;
	}
	const found = actual[section];
	for (const [name, value] of Object.entries(expected)) {
		if (!(name in found))
			failures.push(`${section}: app.css is missing "${name}"`);
		else if (found[name] !== value)
			failures.push(
				`${section}: "${name}" expected "${value}" but app.css has "${found[name]}"`,
			);
	}
	for (const name of Object.keys(found)) {
		if (!(name in expected))
			failures.push(
				`${section}: app.css declares "${name}" which is absent from design/tokens.json`,
			);
	}
}

if (!tokens.desktop?.fontFamily)
	failures.push('design/tokens.json is missing desktop.fontFamily');

if (failures.length > 0) {
	console.error(`design token check failed (${failures.length} problem(s)):`);
	for (const failure of failures) console.error(`  ${failure}`);
	process.exit(1);
}

const count = Object.values(actual).reduce((sum, part) => sum + Object.keys(part).length, 0);
console.log(`design token check passed: ${count} variables across 4 CSS blocks`);

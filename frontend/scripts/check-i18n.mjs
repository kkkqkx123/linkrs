#!/usr/bin/env node
/**
 * Fails when a locale catalogue is out of sync or when the code references a
 * message key that no catalogue defines.
 */
import { readFileSync, readdirSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('..', import.meta.url));
const localesDir = join(root, 'src/lib/i18n/locales');
const srcDir = join(root, 'src');
const referenceLocale = 'en';

const i18nCall = /(?:\$\s*(?:t|format|json)\s*\(|get\(\s*t\s*\)\s*\()/g;
const keyField = /\b(?:label|labelKey|messageKey)\s*:\s*/g;
const stringLiteral = /['"`]([a-z][a-zA-Z0-9]*(?:\.[a-zA-Z0-9${}]+)+)['"`]/g;
const dynamicPrefix = /`([a-z][a-zA-Z0-9]*\.[a-zA-Z0-9]*)\.\$\{/g;

function argumentsOf(source, openParen) {
	let depth = 0;
	for (let i = openParen; i < source.length; i += 1) {
		if (source[i] === '(') depth += 1;
		else if (source[i] === ')') {
			depth -= 1;
			if (depth === 0) return source.slice(openParen + 1, i);
		}
	}
	return source.slice(openParen + 1);
}

function regions(source) {
	const spans = [];
	for (const match of source.matchAll(i18nCall)) {
		spans.push(argumentsOf(source, match.index + match[0].length - 1));
	}
	for (const match of source.matchAll(keyField)) {
		spans.push(
			source.slice(
				match.index + match[0].length,
				match.index + match[0].length + 80,
			),
		);
	}
	return spans;
}

function walk(dir) {
	return readdirSync(dir).flatMap((entry) => {
		const path = join(dir, entry);
		if (statSync(path).isDirectory()) return walk(path);
		return /\.(svelte|ts)$/.test(entry) ? [path] : [];
	});
}

function flatten(node, prefix = '') {
	const out = new Map();
	for (const [key, value] of Object.entries(node)) {
		const path = prefix ? `${prefix}.${key}` : key;
		if (value !== null && typeof value === 'object') {
			for (const [k, v] of flatten(value, path)) out.set(k, v);
		} else if (typeof value !== 'string') {
			throw new Error(
				`message "${path}" must be a string, found ${typeof value}`,
			);
		} else {
			out.set(path, value);
		}
	}
	return out;
}

const catalogues = new Map();
for (const file of readdirSync(localesDir).filter((name) =>
	name.endsWith('.json'),
)) {
	catalogues.set(
		file.replace(/\.json$/, ''),
		flatten(JSON.parse(readFileSync(join(localesDir, file), 'utf8'))),
	);
}
const reference = catalogues.get(referenceLocale);
if (!reference)
	throw new Error(`missing reference catalogue ${referenceLocale}.json`);

const sources = walk(srcDir).map((path) => ({
	path: relative(root, path),
	text: readFileSync(path, 'utf8'),
}));

const referenced = new Set();
const dynamicPrefixes = new Set();
for (const { text } of sources) {
	for (const span of regions(text)) {
		for (const match of span.matchAll(stringLiteral)) referenced.add(match[1]);
		for (const match of text.matchAll(dynamicPrefix))
			dynamicPrefixes.add(`${match[1]}.`);
	}
}

const failures = [];
for (const [locale, catalogue] of catalogues) {
	if (catalogue.size === 0) failures.push(`${locale}.json defines no messages`);
	for (const key of reference.keys())
		if (!catalogue.has(key))
			failures.push(`${locale}.json is missing "${key}"`);
	for (const [key, message] of catalogue) {
		if (!reference.has(key))
			failures.push(`${locale}.json defines unknown "${key}"`);
		else if (locale !== referenceLocale && !message.trim())
			failures.push(`${locale}.json has empty message "${key}"`);
	}
}

const coversDynamically = (key) =>
	[...dynamicPrefixes].some((prefix) => key.startsWith(prefix));
for (const key of referenced) {
	if (!reference.has(key) && !coversDynamically(key))
		failures.push(`no catalogue defines referenced key "${key}"`);
}

if (failures.length > 0) {
	console.error(`i18n check failed (${failures.length} problem(s)):`);
	for (const failure of failures) console.error(`  ${failure}`);
	process.exit(1);
}

const resolved = [...dynamicPrefixes]
	.filter((prefix) =>
		[...reference.keys()].some((key) => key.startsWith(prefix)),
	)
	.sort();

console.log(
	`i18n check passed: ${reference.size} messages, ${catalogues.size} locales, ${sources.length} sources`,
);
if (resolved.length > 0)
	console.log(`  dynamic key prefixes: ${resolved.join(', ')}`);

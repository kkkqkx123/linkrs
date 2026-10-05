#!/usr/bin/env node
/**
 * Fails when a locale catalogue is out of sync with the reference catalogue.
 *
 * Key existence in code is enforced by the generated Paraglide message types,
 * so only catalogue-level consistency is checked here.
 */
import { readFileSync, readdirSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = fileURLToPath(new URL('..', import.meta.url));
const messagesDir = join(root, 'messages');
const referenceLocale = 'en';

const catalogues = new Map();
for (const file of readdirSync(messagesDir).filter((name) =>
	name.endsWith('.json'),
)) {
	const locale = file.replace(/\.json$/, '');
	const messages = JSON.parse(readFileSync(join(messagesDir, file), 'utf8'));
	for (const [key, value] of Object.entries(messages)) {
		if (typeof value !== 'string') {
			throw new Error(
				`message "${key}" in ${file} must be a string, found ${typeof value}`,
			);
		}
	}
	catalogues.set(locale, new Map(Object.entries(messages)));
}

const reference = catalogues.get(referenceLocale);
if (!reference)
	throw new Error(`missing reference catalogue ${referenceLocale}.json`);

const failures = [];
for (const [locale, catalogue] of catalogues) {
	if (catalogue.size === 0) failures.push(`${locale}.json defines no messages`);
	for (const key of reference.keys()) {
		if (!catalogue.has(key))
			failures.push(`${locale}.json is missing "${key}"`);
	}
	for (const [key, message] of catalogue) {
		if (!reference.has(key))
			failures.push(`${locale}.json defines unknown "${key}"`);
		else if (locale !== referenceLocale && !message.trim()) {
			failures.push(`${locale}.json has empty message "${key}"`);
		}
	}
}

if (failures.length > 0) {
	console.error(`i18n check failed (${failures.length} problem(s)):`);
	for (const failure of failures) console.error(`  ${failure}`);
	process.exit(1);
}

console.log(
	`i18n check passed: ${reference.size} messages, ${catalogues.size} locales (${[...catalogues.keys()].join(', ')})`,
);

import { execFile } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, rmSync, statSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { promisify } from 'node:util';
import type { Plugin } from 'vite';

const frontend = fileURLToPath(new URL('..', import.meta.url));
const grammar = resolve(frontend, '..', 'crates', 'linkrs-grammar');
const target = join(
	frontend,
	'src',
	'lib',
	'assets',
	'tree-sitter-linkrs.wasm',
);

const execFileAsync = promisify(execFile);

/**
 * Locate the tree-sitter CLI. The pinned CLI of the grammar crate wins over
 * the one on the path, so a regeneration uses the version the crate was
 * verified with.
 */
function resolveCli(): string {
	const pinned = join(grammar, 'node_modules', '.bin', 'tree-sitter');
	return existsSync(pinned) ? pinned : 'tree-sitter';
}

/**
 * Sources that feed the wasm module. The highlight query is loaded
 * separately as raw text, so it is intentionally not listed here.
 */
const sources = [
	'grammar.js',
	'tree-sitter.json',
	'abi-version',
	'package.json',
	'package-lock.json',
	join('data', 'keywords.json'),
	join('src', 'parser.c'),
	join('src', 'grammar.json'),
	join('src', 'node-types.json'),
];

/**
 * Decide whether the existing wasm output is newer than every source.
 * A missing file counts as stale so the build falls through to the
 * compiler instead of silently reusing an outdated module.
 */
function isFresh(): boolean {
	if (process.env.LINKRS_GRAMMAR_WASM === 'force') return false;
	let targetTime: number;
	try {
		targetTime = statSync(target).mtimeMs;
	} catch {
		return false;
	}
	for (const source of sources) {
		try {
			if (statSync(join(grammar, source)).mtimeMs > targetTime) return false;
		} catch {
			return false;
		}
	}
	return true;
}

/**
 * Compile the grammar parser of the grammar crate into a wasm module for the
 * editor. The module is a build output and therefore not tracked in git.
 */
export async function buildGrammarWasm(): Promise<void> {
	const scratch = join(tmpdir(), `linkrs-grammar-wasm-${process.pid}.wasm`);
	rmSync(scratch, { force: true });
	try {
		await execFileAsync(resolveCli(), ['build', '--wasm', '-o', scratch], {
			cwd: grammar,
		});
		mkdirSync(dirname(target), { recursive: true });
		copyFileSync(scratch, target);
	} catch (error) {
		throw new Error(
			`failed to build the grammar wasm module: ${(error as Error).message}\n` +
				`Install the tree-sitter CLI (npm --prefix crates/linkrs-grammar install) ` +
				`or put it on the path.`,
			{ cause: error },
		);
	} finally {
		rmSync(scratch, { force: true });
	}
}

/**
 * Ensure the grammar wasm module before the bundle is assembled, so a missing
 * grammar fails the build instead of the editor at runtime. Rebuilds only
 * when a grammar source is newer than the output; set
 * LINKRS_GRAMMAR_WASM=force to rebuild unconditionally.
 */
export function grammarWasmPlugin(): Plugin {
	return {
		name: 'linkrs-grammar-wasm',
		async buildStart() {
			if (isFresh()) return;
			await buildGrammarWasm();
		},
	};
}

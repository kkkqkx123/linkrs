import { execFileSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
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
 * Compile the grammar parser of the grammar crate into a wasm module for the
 * editor. The module is a build output and therefore not tracked in git.
 */
export function buildGrammarWasm(): void {
	const scratch = join(tmpdir(), `linkrs-grammar-wasm-${process.pid}.wasm`);
	rmSync(scratch, { force: true });
	try {
		execFileSync(resolveCli(), ['build', '--wasm', '-o', scratch], {
			cwd: grammar,
			stdio: 'inherit',
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
 * Build the grammar wasm module before the bundle is assembled, so a missing
 * grammar fails the build instead of the editor at runtime.
 */
export function grammarWasmPlugin(): Plugin {
	return {
		name: 'linkrs-grammar-wasm',
		buildStart() {
			buildGrammarWasm();
		},
	};
}

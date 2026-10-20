import * as monaco from 'monaco-editor/editor/editor.api.js';
import { Parser, Language, Query } from 'web-tree-sitter';
import highlightsSource from '../../../../crates/linkrs-grammar/queries/highlights.scm?raw';
import grammarWasmUrl from '../assets/tree-sitter-linkrs.wasm?url';

const TOKEN_TYPES = [
	'comment',
	'keyword',
	'operator',
	'number',
	'string',
	'variable',
	'property',
	'function',
] as const;

/**
 * Bare identifiers are classified as variables by the grammar, but the
 * tokenizer of the editor knows which names are tags, edge types or
 * properties. Leaving identifiers to the tokenizer keeps that schema aware
 * colouring while the grammar still decides about keywords, operators and
 * literals.
 */
const IDENTIFIER_NODE_TYPES = new Set(['identifier', 'backtick_identifier']);

let initPromise: Promise<void> | null = null;
let parser: Parser | null = null;
let language: Language | null = null;
let query: Query | null = null;

let cachedVersionId = -1;
let cachedTokens: Uint32Array = new Uint32Array();

async function ensureReady(): Promise<boolean> {
	if (!initPromise) {
		initPromise = (async () => {
			await Parser.init();
			language = await Language.load(grammarWasmUrl);
			parser = new Parser();
			parser.setLanguage(language);
			query = new Query(language, highlightsSource);
		})().catch((error) => {
			// The rejection is kept on purpose: a missing grammar asset must
			// not start a new fetch on every keystroke.
			console.error('tree-sitter initialisation failed', error);
			throw error;
		});
	}
	try {
		await initPromise;
	} catch {
		return false;
	}
	return parser !== null && language !== null && query !== null;
}

function captureTypeIndex(name: string): number {
	const normalized = name.replace(/^@/, '');
	switch (normalized) {
		case 'comment':
			return 0;
		case 'keyword':
			return 1;
		case 'operator':
			return 2;
		case 'number':
			return 3;
		case 'string':
			return 4;
		case 'variable':
			return 5;
		case 'property':
			return 6;
		case 'function':
			return 7;
		default:
			return -1;
	}
}

interface SemanticToken {
	start: number;
	end: number;
	type: number;
}

function collectTokens(model: monaco.editor.ITextModel): SemanticToken[] {
	const tree = parser!.parse(model.getValue());
	if (!tree) return [];
	try {
		const captures = query!.captures(tree.rootNode);
		const items: SemanticToken[] = [];
		for (const capture of captures) {
			const type = captureTypeIndex(capture.name);
			if (type < 0) continue;
			const node = capture.node;
			if (node.startIndex >= node.endIndex) continue;
			if (capture.name === 'variable' && IDENTIFIER_NODE_TYPES.has(node.type)) {
				continue;
			}
			items.push({ start: node.startIndex, end: node.endIndex, type });
		}
		items.sort((a, b) => a.start - b.start || a.end - b.end);

		const filtered: SemanticToken[] = [];
		let lastEnd = -1;
		for (const item of items) {
			if (item.start < lastEnd) continue;
			filtered.push(item);
			lastEnd = item.end;
		}
		return filtered;
	} finally {
		tree.delete();
	}
}

function encodeTokens(
	model: monaco.editor.ITextModel,
	items: SemanticToken[],
): Uint32Array {
	const data: number[] = [];
	let prevLine = 0;
	let prevChar = 0;
	for (const item of items) {
		const start = model.getPositionAt(item.start);
		const line = start.lineNumber - 1;
		const char = start.column - 1;
		const deltaLine = line - prevLine;
		const deltaStart = deltaLine === 0 ? char - prevChar : char;
		data.push(deltaLine, deltaStart, item.end - item.start, item.type, 0);
		prevLine = line;
		prevChar = char;
	}
	return new Uint32Array(data);
}

/**
 * Register the grammar driven highlighting for the given language.
 *
 * The grammar is loaded from the wasm module that the grammar crate builds,
 * so the editor always highlights with the same grammar the backend parses
 * with.
 */
export function registerTreeSitterHighlighting(
	monacoInstance: typeof monaco,
	languageId: string,
): monaco.IDisposable {
	const legend: monaco.languages.SemanticTokensLegend = {
		tokenTypes: [...TOKEN_TYPES],
		tokenModifiers: [],
	};

	return monacoInstance.languages.registerDocumentSemanticTokensProvider(
		{ language: languageId },
		{
			getLegend: () => legend,
			provideDocumentSemanticTokens: async (model) => {
				const ready = await ensureReady();
				if (!ready || !parser || !query) {
					return { data: new Uint32Array() };
				}
				const versionId = model.getVersionId();
				if (versionId === cachedVersionId) {
					return { data: cachedTokens, resultId: String(versionId) };
				}
				const data = encodeTokens(model, collectTokens(model));
				cachedVersionId = versionId;
				cachedTokens = data;
				return { data, resultId: String(versionId) };
			},
			releaseDocumentSemanticTokens: () => {},
		},
	);
}

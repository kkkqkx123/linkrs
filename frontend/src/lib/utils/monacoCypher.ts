import type * as Monaco from 'monaco-editor/editor/editor.api.js';
import type { Tag, EdgeType } from '$types/schema';

export const CYPHER_LANGUAGE_ID = 'cypher';

export const CYPHER_KEYWORDS = [
	'MATCH',
	'OPTIONAL',
	'WHERE',
	'RETURN',
	'WITH',
	'GO',
	'CREATE',
	'DELETE',
	'DETACH',
	'SET',
	'REMOVE',
	'MERGE',
	'UNWIND',
	'CALL',
	'YIELD',
	'ORDER',
	'BY',
	'LIMIT',
	'SKIP',
	'COUNT',
	'AS',
	'AND',
	'OR',
	'NOT',
	'IN',
	'TAG',
	'EDGE',
	'SPACE',
	'USE',
	'VERTEX',
	'INDEX',
	'ALTER',
	'DROP',
	'FETCH',
	'PROP',
	'ON',
	'LOOKUP',
	'STARTS',
	'ENDS',
	'CONTAINS',
	'DISTINCT',
	'IF',
	'EXISTS',
	'ASC',
	'DESC',
	'SHOW',
	'DESCRIBE',
	'ADD',
	'REBUILD',
	'DEFAULT',
];

export const CYPHER_FUNCTIONS = [
	'count',
	'sum',
	'avg',
	'min',
	'max',
	'id',
	'keys',
	'labels',
	'type',
	'exists',
	'toInteger',
	'toString',
	'toFloat',
	'abs',
	'ceil',
	'floor',
	'round',
	'length',
	'substring',
	'toUpper',
	'toLower',
	'trim',
	'split',
	'replace',
	'size',
	'head',
	'last',
	'range',
	'coalesce',
];

/**
 * Completion ordering. Monaco sorts suggestions by `sortText`, so a lower
 * group number pushes the whole category to the top of the list.
 */
const SORT_GROUP = {
	keyword: '1',
	tag: '2',
	edge: '3',
	field: '4',
	function: '5',
} as const;

let languageRegistered = false;
let tokensDisposable: Monaco.IDisposable | null = null;

/** Snapshot accessor shared by the completion engine and highlight refresh. */
export type SchemaSnapshotProvider = () => {
	tags: Tag[];
	edgeTypes: EdgeType[];
};

interface SchemaSnapshot {
	tags: Tag[];
	edgeTypes: EdgeType[];
}

function readSchema(getSchema: SchemaSnapshotProvider): SchemaSnapshot {
	try {
		const snapshot = getSchema();
		return { tags: snapshot.tags ?? [], edgeTypes: snapshot.edgeTypes ?? [] };
	} catch {
		// Schema may be unavailable; keyword and function completion still works.
		return { tags: [], edgeTypes: [] };
	}
}

/** Escape a schema name so it is safe to embed inside a RegExp alternation. */
function escapeRegex(value: string): string {
	return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/** Wrap a schema name in backticks when it is not a plain identifier. */
function quoteIfNeeded(value: string): string {
	return /^[A-Za-z_]\w*$/.test(value) &&
		!CYPHER_KEYWORDS.includes(value.toUpperCase())
		? value
		: '`' + value.replace(/`/g, '') + '`';
}

/**
 * Build Monarch highlight rules that colour tag and edge names distinctly.
 * Names are matched only at word boundaries so substrings of larger
 * identifiers are left untouched.
 */
function buildSchemaRules(
	names: string[],
	token: string,
): Monaco.languages.IMonarchLanguageRule[] {
	if (names.length === 0) return [];
	const pattern = names.map(escapeRegex).join('|');
	return [[new RegExp(`\\b(?:${pattern})\\b`), token]];
}

/** Collect property names that are safe to highlight as fields. */
function collectFieldNames(schema: SchemaSnapshot): string[] {
	const blocked = new Set([
		...CYPHER_KEYWORDS.map((keyword) => keyword.toLowerCase()),
		...CYPHER_FUNCTIONS.map((fn) => fn.toLowerCase()),
	]);
	const seen = new Set<string>();
	const out: string[] = [];
	const consider = (name: string) => {
		const key = name.toLowerCase();
		if (!name || blocked.has(key) || seen.has(key)) return;
		seen.add(key);
		out.push(name);
	};
	for (const tag of schema.tags) {
		for (const prop of tag.properties ?? []) consider(prop.name);
	}
	for (const edge of schema.edgeTypes) {
		for (const prop of edge.properties ?? []) consider(prop.name);
	}
	return out;
}

/** The shared tokenizer body: schema rules are injected at the front by the refresh step. */
function createTokenizer(
	schema: SchemaSnapshot,
): Monaco.languages.IMonarchLanguage {
	const tags = schema.tags.map((t) => t.name);
	const edges = schema.edgeTypes.map((e) => e.name);
	const fields = collectFieldNames(schema);
	return {
		defaultToken: '',
		ignoreCase: true,
		keywords: CYPHER_KEYWORDS,
		functions: CYPHER_FUNCTIONS,
		operators: [
			'==',
			'!=',
			'<>',
			'<=',
			'>=',
			'=',
			'<',
			'>',
			'+',
			'-',
			'*',
			'/',
			'%',
		],
		tokenizer: {
			root: [
				...buildSchemaRules(tags, 'tag'),
				...buildSchemaRules(edges, 'edge'),
				...buildSchemaRules(fields, 'field'),
				[/--.*$/, 'comment'],
				[/\/\/.*$/, 'comment'],
				[/#.*$/, 'comment'],
				[/\/\*/, 'comment', '@comment'],
				[/'([^'\\]|\\.)*'/, 'string'],
				[/"([^"\\]|\\.)*"/, 'string'],
				[/`([^`\\]|\\.)*`/, 'identifier'],
				[
					/[a-zA-Z_]\w*/,
					{
						cases: {
							'@keywords': 'keyword',
							'@functions': 'type.identifier',
							'@default': 'identifier',
						},
					},
				],
				[/\d+\.\d+([eE][-+]?\d+)?/, 'number.float'],
				[/0[xX][0-9a-fA-F]+/, 'number.hex'],
				[/\d+/, 'number'],
				[/[(){}[\]]/, '@brackets'],
				[/[;,]/, 'delimiter'],
				[/[=<>!+\-*/%]/, 'operator'],
			],
			comment: [
				[/[^/*]+/, 'comment'],
				[/\*\//, 'comment', '@pop'],
				[/[/*]/, 'comment'],
			],
		},
	};
}

/**
 * Register the Cypher language and install its tokenizer. Monarch
 * highlights in the UI thread only, so no language web worker is required.
 */
export function registerCypherLanguage(monaco: typeof Monaco): void {
	if (
		languageRegistered ||
		monaco.languages
			.getLanguages()
			.some((lang) => lang.id === CYPHER_LANGUAGE_ID)
	) {
		languageRegistered = true;
		return;
	}

	monaco.languages.register({
		id: CYPHER_LANGUAGE_ID,
		extensions: ['.cypher', '.cql'],
	});

	tokensDisposable?.dispose();
	tokensDisposable = monaco.languages.setMonarchTokensProvider(
		CYPHER_LANGUAGE_ID,
		createTokenizer({ tags: [], edgeTypes: [] }),
	);

	monaco.languages.setLanguageConfiguration(CYPHER_LANGUAGE_ID, {
		comments: { lineComment: '--', blockComment: ['/*', '*/'] },
		brackets: [
			['(', ')'],
			['[', ']'],
			['{', '}'],
		],
		autoClosingPairs: [
			{ open: '(', close: ')' },
			{ open: '[', close: ']' },
			{ open: '{', close: '}' },
			{ open: "'", close: "'" },
			{ open: '"', close: '"' },
			{ open: '`', close: '`' },
		],
	});

	languageRegistered = true;
}

/**
 * Refresh schema-driven highlighting by re-installing the tokenizer with
 * tag/edge names as first-class tokens. Called whenever the schema changes so
 * renamed or newly created entities pick up their colour immediately.
 */
export function updateSchemaHighlighting(
	monaco: typeof Monaco,
	schema: SchemaSnapshot,
): void {
	if (!languageRegistered) return;
	const next = createTokenizer(schema);
	tokensDisposable?.dispose();
	tokensDisposable = monaco.languages.setMonarchTokensProvider(
		CYPHER_LANGUAGE_ID,
		next,
	);
}

/** Range of the word being typed, used as the replacement span for a suggestion. */
function wordRange(
	model: Monaco.editor.ITextModel,
	position: Monaco.Position,
): Monaco.IRange {
	const word = model.getWordUntilPosition(position);
	return {
		startLineNumber: position.lineNumber,
		endLineNumber: position.lineNumber,
		startColumn: word.startColumn,
		endColumn: word.endColumn,
	};
}

function buildKeywordItems(
	monaco: typeof Monaco,
	range: Monaco.IRange,
): Monaco.languages.CompletionItem[] {
	return CYPHER_KEYWORDS.map((keyword) => ({
		label: keyword,
		kind: monaco.languages.CompletionItemKind.Keyword,
		insertText: keyword,
		sortText: SORT_GROUP.keyword,
		range,
	}));
}

function buildFunctionItems(
	monaco: typeof Monaco,
	range: Monaco.IRange,
): Monaco.languages.CompletionItem[] {
	return CYPHER_FUNCTIONS.map((fn) => ({
		label: fn,
		kind: monaco.languages.CompletionItemKind.Function,
		insertText: `${fn}($0)`,
		insertTextRules:
			monaco.languages.CompletionItemInsertTextRule.InsertAsSnippet,
		sortText: SORT_GROUP.function,
		range,
	}));
}

function buildEntityItems(
	monaco: typeof Monaco,
	schema: SchemaSnapshot,
	range: Monaco.IRange,
): Monaco.languages.CompletionItem[] {
	const items: Monaco.languages.CompletionItem[] = [];
	for (const tag of schema.tags) {
		items.push({
			label: tag.name,
			kind: monaco.languages.CompletionItemKind.Class,
			detail: 'tag',
			insertText: quoteIfNeeded(tag.name),
			sortText: SORT_GROUP.tag,
			range,
		});
	}
	for (const edge of schema.edgeTypes) {
		items.push({
			label: edge.name,
			kind: monaco.languages.CompletionItemKind.Interface,
			detail: 'edge type',
			insertText: quoteIfNeeded(edge.name),
			sortText: SORT_GROUP.edge,
			range,
		});
	}
	return items;
}

/**
 * Map query variable aliases to the tag or edge type they bind.
 * Only the common inline forms are recognized; anything else falls back
 * to literal entity name matching in the property provider.
 */
function buildVariableEntityMap(text: string): Map<string, string> {
	const mapping = new Map<string, string>();
	const nodePattern = /\(\s*([A-Za-z_]\w*)\s*:\s*([A-Za-z_]\w*)/g;
	let match: RegExpExecArray | null;
	while ((match = nodePattern.exec(text)) !== null) {
		mapping.set(match[1].toLowerCase(), match[2].toLowerCase());
	}
	const edgePattern = /\[\s*([A-Za-z_]\w*)\s*:\s*([A-Za-z_]\w*)/g;
	while ((match = edgePattern.exec(text)) !== null) {
		mapping.set(match[1].toLowerCase(), match[2].toLowerCase());
	}
	return mapping;
}

/** Collect distinct parameter names for one prefix from the current text. */
function collectParamNames(text: string, prefix: string): string[] {
	const pattern = prefix === '$' ? /\$([A-Za-z_]\w*)/g : /@([A-Za-z_]\w*)/g;
	const names: string[] = [];
	let match: RegExpExecArray | null;
	while ((match = pattern.exec(text)) !== null) {
		if (!names.includes(match[1])) names.push(match[1]);
	}
	return names;
}

/**
 * Register completion providers for the Cypher console.
 *
 * Several narrow providers are used instead of one flat list so each can
 * declare its own trigger characters and ordering. The field provider inspects
 * the identifier before the caret to suggest only the properties of the tag or
 * edge named there.
 */
export function registerCypherCompletions(
	monaco: typeof Monaco,
	getSchema: SchemaSnapshotProvider,
): Monaco.IDisposable {
	const disposables: Monaco.IDisposable[] = [];

	// Keywords and functions: always available, no trigger characters.
	disposables.push(
		monaco.languages.registerCompletionItemProvider(CYPHER_LANGUAGE_ID, {
			provideCompletionItems: (model, position) => {
				const range = wordRange(model, position);
				return {
					suggestions: [
						...buildKeywordItems(monaco, range),
						...buildFunctionItems(monaco, range),
					],
				};
			},
		}),
	);

	// Tags and edge types: available everywhere, and again right after ':'.
	disposables.push(
		monaco.languages.registerCompletionItemProvider(CYPHER_LANGUAGE_ID, {
			triggerCharacters: [':'],
			provideCompletionItems: (model, position) => {
				const range = wordRange(model, position);
				return {
					suggestions: buildEntityItems(monaco, readSchema(getSchema), range),
				};
			},
		}),
	);

	// Properties: triggered by '.', filtered by the entity name before the dot.
	disposables.push(
		monaco.languages.registerCompletionItemProvider(CYPHER_LANGUAGE_ID, {
			triggerCharacters: ['.'],
			provideCompletionItems: (model, position) => {
				const range = wordRange(model, position);
				const schema = readSchema(getSchema);
				// Inspect the identifier immediately preceding the dot to scope fields.
				const before = model.getValueInRange({
					startLineNumber: position.lineNumber,
					startColumn: 1,
					endLineNumber: position.lineNumber,
					endColumn: position.column,
				});
				const match = before.match(/([A-Za-z_]\w*)\.\s*\w*$/);
				const owner = match?.[1]?.toLowerCase();
				const aliasTarget = owner
					? buildVariableEntityMap(model.getValue()).get(owner)
					: undefined;
				const effectiveOwner = aliasTarget ?? owner;
				const suggestions: Monaco.languages.CompletionItem[] = [];
				const pushProps = (
					parent: string,
					props: { name: string; data_type?: string }[],
				) => {
					for (const prop of props) {
						suggestions.push({
							label: prop.name,
							kind: monaco.languages.CompletionItemKind.Field,
							detail: `${parent}.${prop.name}${prop.data_type ? ` (${prop.data_type})` : ''}`,
							insertText: prop.name,
							sortText: SORT_GROUP.field,
							range,
						});
					}
				};
				for (const tag of schema.tags) {
					if (!owner || tag.name.toLowerCase() === effectiveOwner)
						pushProps(tag.name, tag.properties ?? []);
				}
				for (const edge of schema.edgeTypes) {
					if (!owner || edge.name.toLowerCase() === effectiveOwner)
						pushProps(edge.name, edge.properties ?? []);
				}
				return { suggestions };
			},
		}),
	);

	// Parameters: triggered by '$' and '@', sourced from names already present.
	disposables.push(
		monaco.languages.registerCompletionItemProvider(CYPHER_LANGUAGE_ID, {
			triggerCharacters: ['$', '@'],
			provideCompletionItems: (model, position) => {
				const before = model.getValueInRange({
					startLineNumber: position.lineNumber,
					startColumn: 1,
					endLineNumber: position.lineNumber,
					endColumn: position.column,
				});
				const trigger = before.match(/([@$])([A-Za-z_]\w*)?$/);
				if (!trigger) return { suggestions: [] };
				const prefix = trigger[1];
				const range = wordRange(model, position);
				const names = collectParamNames(model.getValue(), prefix);
				return {
					suggestions: names.map((name) => ({
						label: `${prefix}${name}`,
						kind: monaco.languages.CompletionItemKind.Variable,
						detail: prefix === '$' ? 'session variable' : 'parameter',
						filterText: name,
						insertText: name,
						sortText: SORT_GROUP.keyword,
						range,
					})),
				};
			},
		}),
	);

	return {
		dispose: () => {
			for (const disposable of disposables) disposable.dispose();
		},
	};
}

/**
 * Register slash-triggered history completion for the Cypher console.
 * Typing `/` followed by a prefix offers past statements whose text contains
 * the prefix; accepting replaces the slash filter with the full statement.
 */
export function registerHistoryCompletion(
	monaco: typeof Monaco,
	getHistory: () => string[],
): Monaco.IDisposable {
	return monaco.languages.registerCompletionItemProvider(CYPHER_LANGUAGE_ID, {
		triggerCharacters: ['/'],
		provideCompletionItems: (model, position) => {
			const before = model.getValueInRange({
				startLineNumber: position.lineNumber,
				startColumn: 1,
				endLineNumber: position.lineNumber,
				endColumn: position.column,
			});
			const trigger = before.match(/\/([A-Za-z0-9_]*)$/);
			if (!trigger) return { suggestions: [] };
			const prefix = trigger[1].toLowerCase();
			const range = {
				startLineNumber: position.lineNumber,
				startColumn: position.column - trigger[0].length,
				endLineNumber: position.lineNumber,
				endColumn: position.column,
			};
			const seen = new Set<string>();
			const suggestions: Monaco.languages.CompletionItem[] = [];
			for (const entry of getHistory()) {
				const statement = entry.trim();
				if (!statement || seen.has(statement)) continue;
				seen.add(statement);
				if (prefix && !statement.toLowerCase().includes(prefix)) continue;
				const headline =
					statement.length > 80 ? `${statement.slice(0, 80)}...` : statement;
				suggestions.push({
					label: headline,
					kind: monaco.languages.CompletionItemKind.Text,
					detail: 'history',
					filterText: statement,
					insertText: statement,
					sortText: SORT_GROUP.keyword,
					range,
				});
				if (suggestions.length >= 20) break;
			}
			return { suggestions };
		},
	});
}

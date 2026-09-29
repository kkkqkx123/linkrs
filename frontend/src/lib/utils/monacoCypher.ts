import type * as Monaco from 'monaco-editor/editor/editor.api.js';
import type { Tag, EdgeType } from '$types/schema';

export const CYPHER_LANGUAGE_ID = 'cypher';

export const CYPHER_KEYWORDS = [
  'MATCH', 'OPTIONAL', 'WHERE', 'RETURN', 'WITH', 'GO', 'CREATE', 'DELETE',
  'DETACH', 'SET', 'REMOVE', 'MERGE', 'UNWIND', 'CALL', 'YIELD', 'ORDER',
  'BY', 'LIMIT', 'SKIP', 'COUNT', 'AS', 'AND', 'OR', 'NOT', 'IN', 'TAG',
  'EDGE', 'SPACE', 'USE', 'VERTEX', 'INDEX', 'ALTER', 'DROP', 'FETCH', 'PROP',
  'ON', 'LOOKUP', 'STARTS', 'ENDS', 'CONTAINS', 'DISTINCT', 'IF', 'EXISTS',
  'ASC', 'DESC', 'SHOW', 'DESCRIBE', 'ADD', 'REBUILD', 'DEFAULT',
];

export const CYPHER_FUNCTIONS = [
  'count', 'sum', 'avg', 'min', 'max', 'id', 'keys', 'labels', 'type',
  'exists', 'toInteger', 'toString', 'toFloat', 'abs', 'ceil', 'floor',
  'round', 'length', 'substring', 'toUpper', 'toLower', 'trim', 'split',
  'replace', 'size', 'head', 'last', 'range', 'coalesce',
];

let languageRegistered = false;

// Register the Cypher language and its Monarch tokenizer exactly once. Monarch
// handles highlighting purely in the UI thread, so no language web worker is
// required.
export function registerCypherLanguage(monaco: typeof Monaco): void {
  if (languageRegistered || monaco.languages.getLanguages().some((lang) => lang.id === CYPHER_LANGUAGE_ID)) {
    languageRegistered = true;
    return;
  }

  monaco.languages.register({ id: CYPHER_LANGUAGE_ID, extensions: ['.cypher', '.cql'] });

  monaco.languages.setMonarchTokensProvider(CYPHER_LANGUAGE_ID, {
    defaultToken: '',
    ignoreCase: true,
    keywords: CYPHER_KEYWORDS,
    functions: CYPHER_FUNCTIONS,
    operators: ['==', '!=', '<>', '<=', '>=', '=', '<', '>', '+', '-', '*', '/', '%'],
    tokenizer: {
      root: [
        [/--.*$/, 'comment'],
        [/\/\/.*$/, 'comment'],
        [/\/\*/, 'comment', '@comment'],
        [/'([^'\\]|\\.)*'/, 'string'],
        [/"([^"\\]|\\.)*"/, 'string'],
        [/`([^`\\]|\\.)*`/, 'identifier'],
        [/[a-zA-Z_]\w*/, {
          cases: {
            '@keywords': 'keyword',
            '@functions': 'type.identifier',
            '@default': 'identifier',
          },
        }],
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
  });

  monaco.languages.setLanguageConfiguration(CYPHER_LANGUAGE_ID, {
    comments: { lineComment: '--', blockComment: ['/*', '*/'] },
    brackets: [['(', ')'], ['[', ']'], ['{', '}']],
    autoClosingPairs: [
      { open: '(', close: ')' },
      { open: '[', close: ']' },
      { open: '{', close: '}' },
      { open: "'", close: "'" },
      { open: '"', close: '"' },
    ],
  });

  languageRegistered = true;
}

// Provider used by the completion engine to surface schema-aware suggestions.
export type SchemaSnapshotProvider = () => { tags: Tag[]; edgeTypes: EdgeType[] };

// Register completion items: keywords, functions, and the tags, edge types and
// properties currently known to the schema store.
export function registerCypherCompletions(monaco: typeof Monaco, getSchema: SchemaSnapshotProvider): Monaco.IDisposable {
  return monaco.languages.registerCompletionItemProvider(CYPHER_LANGUAGE_ID, {
    provideCompletionItems: (model, position) => {
      const word = model.getWordUntilPosition(position);
      const range: Monaco.IRange = {
        startLineNumber: position.lineNumber,
        endLineNumber: position.lineNumber,
        startColumn: word.startColumn,
        endColumn: word.endColumn,
      };

      const keywordItems: Monaco.languages.CompletionItem[] = CYPHER_KEYWORDS.map((keyword) => ({
        label: keyword,
        kind: monaco.languages.CompletionItemKind.Keyword,
        insertText: keyword,
        range,
      }));

      const functionItems: Monaco.languages.CompletionItem[] = CYPHER_FUNCTIONS.map((fn) => ({
        label: fn,
        kind: monaco.languages.CompletionItemKind.Function,
        insertText: `${fn}($0)`,
        insertTextRules: monaco.languages.CompletionItemInsertTextRule.InsertAsSnippet,
        range,
      }));

      const schemaItems: Monaco.languages.CompletionItem[] = [];
      try {
        const schema = getSchema();
        for (const tag of schema.tags ?? []) {
          schemaItems.push({
            label: tag.name,
            kind: monaco.languages.CompletionItemKind.Class,
            detail: 'tag',
            insertText: tag.name,
            range,
          });
          for (const prop of tag.properties ?? []) {
            schemaItems.push({
              label: prop.name,
              kind: monaco.languages.CompletionItemKind.Field,
              detail: `tag ${tag.name} property`,
              insertText: prop.name,
              range,
            });
          }
        }
        for (const edge of schema.edgeTypes ?? []) {
          schemaItems.push({
            label: edge.name,
            kind: monaco.languages.CompletionItemKind.Interface,
            detail: 'edge type',
            insertText: edge.name,
            range,
          });
        }
      } catch {
        // Schema may be unavailable; keyword and function completion still work.
      }

      return { suggestions: [...keywordItems, ...functionItems, ...schemaItems] };
    },
  });
}

import { CYPHER_KEYWORDS } from '$utils/monacoCypher';

interface ScannedStatement {
	query: string;
	start: number;
	end: number;
}

function isWhitespace(char: string): boolean {
	return (
		char === ' ' ||
		char === '\t' ||
		char === '\n' ||
		char === '\r' ||
		char === '\f'
	);
}

function scanStatements(content: string): ScannedStatement[] {
	const out: ScannedStatement[] = [];
	let current = '';
	let stmtStart: number | null = null;
	let stmtEnd = 0;
	let stringChar = '';
	let escaped = false;
	let inLineComment = false;
	let inBlockComment = false;

	const appendContent = (
		text: string,
		origIndex: number,
		contentChar: boolean,
	) => {
		if (stmtStart === null && contentChar) stmtStart = origIndex;
		if (contentChar) stmtEnd = origIndex + text.length;
		current += text;
	};

	let i = 0;
	while (i < content.length) {
		const char = content[i];
		const next = content[i + 1] ?? '';
		const next2 = content[i + 2] ?? '';

		if (inBlockComment) {
			if (char === '*' && next === '/') {
				inBlockComment = false;
				i += 2;
				if (current && !isWhitespace(current[current.length - 1]))
					current += ' ';
				continue;
			}
			i += 1;
			continue;
		}

		if (inLineComment) {
			if (char === '\n') {
				inLineComment = false;
				current += char;
				i += 1;
				continue;
			}
			i += 1;
			continue;
		}

		if (stringChar) {
			if (escaped) {
				appendContent(char, i, true);
				escaped = false;
				i += 1;
				continue;
			}
			if (char === '\\') {
				appendContent(char, i, true);
				escaped = true;
				i += 1;
				continue;
			}
			appendContent(char, i, true);
			if (char === stringChar) stringChar = '';
			i += 1;
			continue;
		}

		if (escaped) {
			appendContent(char, i, true);
			escaped = false;
			i += 1;
			continue;
		}

		if (char === '\\') {
			if (next === '\n') {
				i += 2;
				continue;
			}
			if (next === '\r' && next2 === '\n') {
				i += 3;
				continue;
			}
			appendContent(char, i, true);
			escaped = true;
			i += 1;
			continue;
		}

		if (char === '"' || char === "'" || char === '`') {
			stringChar = char;
			appendContent(char, i, true);
			i += 1;
			continue;
		}

		if (char === '/' && next === '*') {
			inBlockComment = true;
			i += 2;
			if (current && !isWhitespace(current[current.length - 1])) current += ' ';
			continue;
		}

		if (
			(char === '-' && next === '-') ||
			(char === '/' && next === '/') ||
			char === '#'
		) {
			inLineComment = true;
			i += char === '#' ? 1 : 2;
			continue;
		}

		if (char === ';') {
			const trimmed = current.trim();
			if (trimmed && stmtStart !== null) {
				out.push({ query: trimmed, start: stmtStart, end: stmtEnd });
			}
			current = '';
			stmtStart = null;
			stmtEnd = 0;
			i += 1;
			continue;
		}

		if (isWhitespace(char)) {
			current += char;
			i += 1;
			continue;
		}

		appendContent(char, i, true);
		i += 1;
	}

	const trimmed = current.trim();
	if (trimmed && stmtStart !== null) {
		out.push({ query: trimmed, start: stmtStart, end: stmtEnd });
	}
	return out;
}

export const splitQueries = (content: string): string[] => {
	if (!content || !content.trim()) return [];
	return scanStatements(content).map((stmt) => stmt.query);
};

/** Clause keywords that start a new line in formatted output. */
const FORMAT_BREAK_BEFORE = new Set([
	'MATCH',
	'OPTIONAL',
	'WHERE',
	'WITH',
	'RETURN',
	'ORDER',
	'SKIP',
	'LIMIT',
	'CREATE',
	'DELETE',
	'DETACH',
	'SET',
	'REMOVE',
	'MERGE',
	'UNWIND',
	'CALL',
	'YIELD',
	'GO',
	'FETCH',
	'LOOKUP',
	'USE',
	'SHOW',
	'DESCRIBE',
	'DROP',
	'ALTER',
	'ADD',
	'REBUILD',
]);

function isWordChar(char: string): boolean {
	return /[A-Za-z0-9_]/.test(char);
}

/**
 * Format query text with uppercased keywords and one major clause per line.
 * Strings, quoted identifiers, and comments pass through byte-for-byte so
 * formatting never changes query semantics.
 */
export const formatQuery = (content: string): string => {
	if (!content || !content.trim()) return content;
	let out = '';
	let word = '';
	let stringChar = '';
	let escaped = false;
	let inLineComment = false;
	let inBlockComment = false;
	let atLineStart = true;

	const keywordSet = new Set(CYPHER_KEYWORDS);

	const flushWord = () => {
		if (!word) return;
		const upper = word.toUpperCase();
		if (keywordSet.has(upper)) {
			if (FORMAT_BREAK_BEFORE.has(upper) && !atLineStart && out.trim()) {
				out += '\n';
				atLineStart = true;
			}
			out += upper;
		} else {
			out += word;
		}
		word = '';
		atLineStart = false;
	};

	let i = 0;
	while (i < content.length) {
		const char = content[i];
		const next = content[i + 1] ?? '';

		if (inBlockComment) {
			out += char;
			if (char === '*' && next === '/') {
				out += next;
				i += 2;
				inBlockComment = false;
				continue;
			}
			if (char === '\n') atLineStart = true;
			else if (!isWhitespace(char)) atLineStart = false;
			i += 1;
			continue;
		}

		if (inLineComment) {
			out += char;
			i += 1;
			if (char === '\n') {
				inLineComment = false;
				atLineStart = true;
			}
			continue;
		}

		if (stringChar) {
			out += char;
			if (escaped) escaped = false;
			else if (char === '\\') escaped = true;
			else if (char === stringChar) stringChar = '';
			i += 1;
			continue;
		}

		if (char === '"' || char === "'" || char === '`') {
			flushWord();
			stringChar = char;
			out += char;
			atLineStart = false;
			i += 1;
			continue;
		}

		if (char === '/' && next === '*') {
			flushWord();
			inBlockComment = true;
			out += '/*';
			i += 2;
			continue;
		}

		if (
			(char === '-' && next === '-') ||
			(char === '/' && next === '/') ||
			char === '#'
		) {
			flushWord();
			inLineComment = true;
			if (char === '#') {
				out += char;
				i += 1;
			} else {
				out += char + next;
				i += 2;
			}
			continue;
		}

		if (isWordChar(char)) {
			word += char;
			i += 1;
			continue;
		}

		flushWord();

		if (char === ';') {
			out += ';';
			atLineStart = false;
			i += 1;
			let j = i;
			while (j < content.length && (content[j] === ' ' || content[j] === '\t'))
				j += 1;
			if (j < content.length && content[j] !== '\n') {
				out += '\n';
				atLineStart = true;
			}
			i = j;
			continue;
		}

		if (isWhitespace(char)) {
			if (char === '\n') {
				if (!out.endsWith('\n')) out += '\n';
				atLineStart = true;
			} else if (!out.endsWith('\n') && !out.endsWith(' ') && out.length > 0) {
				out += ' ';
			}
			i += 1;
			continue;
		}

		out += char;
		atLineStart = false;
		i += 1;
	}
	flushWord();
	return out
		.split('\n')
		.map((line) => line.trimEnd())
		.join('\n')
		.trim();
};

export const getQueryAtCursor = (
	content: string,
	cursorPosition: number,
): { query: string; start: number; end: number } => {
	if (!content) return { query: '', start: 0, end: 0 };
	const statements = scanStatements(content);
	if (statements.length === 0) return { query: '', start: 0, end: 0 };
	const cursor = Math.max(0, Math.min(cursorPosition, content.length));
	for (const stmt of statements) {
		if (cursor >= stmt.start && cursor <= stmt.end) return { ...stmt };
	}
	for (const stmt of statements) {
		if (stmt.start > cursor) return { ...stmt };
	}
	const last = statements[statements.length - 1];
	return { ...last };
};

export type StreamBlockReason = 'empty' | 'command';

export type StreamMode = 'single' | 'batch' | null;

export interface StreamEligibility {
	eligible: boolean;
	reason: StreamBlockReason | null;
	statement: string;
	count: number;
	/** Execution shape: one statement streams alone, several stream as a batch. */
	mode: StreamMode;
	statements: string[];
}

export function isCommandLikeStatement(text: string): boolean {
	const upper = text.trim().toUpperCase();
	return (
		upper === 'BEGIN' ||
		upper.startsWith('BEGIN ') ||
		upper.startsWith('START TRANSACTION') ||
		upper.startsWith('COMMIT') ||
		upper.startsWith('ROLLBACK') ||
		upper.startsWith('SAVEPOINT') ||
		upper.startsWith('RELEASE SAVEPOINT') ||
		upper === 'LET' ||
		upper.startsWith('LET ')
	);
}

export function getStreamEligibility(content: string): StreamEligibility {
	const statements = splitQueries(content);
	if (statements.length === 0) {
		return {
			eligible: false,
			reason: 'empty',
			statement: '',
			count: 0,
			mode: null,
			statements: [],
		};
	}
	if (statements.length > 1) {
		// Multi-statement scripts stream as a batch; per-statement command
		// fallback stays server-side, so no client-side exclusion applies.
		return {
			eligible: true,
			reason: null,
			statement: '',
			count: statements.length,
			mode: 'batch',
			statements,
		};
	}
	const statement = statements[0];
	if (isCommandLikeStatement(statement)) {
		return {
			eligible: false,
			reason: 'command',
			statement,
			count: 1,
			mode: null,
			statements,
		};
	}
	return {
		eligible: true,
		reason: null,
		statement,
		count: 1,
		mode: 'single',
		statements,
	};
}

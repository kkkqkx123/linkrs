interface ScannedStatement {
  query: string;
  start: number;
  end: number;
}

function isWhitespace(char: string): boolean {
  return char === ' ' || char === '\t' || char === '\n' || char === '\r' || char === '\f';
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

  const appendContent = (text: string, origIndex: number, contentChar: boolean) => {
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
        if (current && !isWhitespace(current[current.length - 1])) current += ' ';
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

    if ((char === '-' && next === '-') || (char === '/' && next === '/') || char === '#') {
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

export const getQueryAtCursor = (content: string, cursorPosition: number): { query: string; start: number; end: number } => {
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

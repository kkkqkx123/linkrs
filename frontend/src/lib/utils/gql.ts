export const splitQueries = (content: string): string[] => {
  if (!content || !content.trim()) return [];
  const queries: string[] = [];
  let currentQuery = '';
  let inString = false;
  let stringChar = '';
  let escaped = false;

  for (let i = 0; i < content.length; i++) {
    const char = content[i];
    if (escaped) { currentQuery += char; escaped = false; continue; }
    if (char === '\\') { currentQuery += char; escaped = true; continue; }
    if (!inString && (char === '"' || char === "'" || char === '`')) {
      inString = true; stringChar = char; currentQuery += char; continue;
    }
    if (inString && char === stringChar) {
      inString = false; stringChar = ''; currentQuery += char; continue;
    }
    if (!inString && char === ';') {
      const trimmed = currentQuery.trim();
      if (trimmed) queries.push(trimmed);
      currentQuery = '';
      continue;
    }
    currentQuery += char;
  }
  const trimmed = currentQuery.trim();
  if (trimmed) queries.push(trimmed);
  return queries;
};

export const getQueryAtCursor = (content: string, cursorPosition: number): { query: string; start: number; end: number } => {
  if (!content) return { query: '', start: 0, end: 0 };
  const queries = splitQueries(content);
  let currentPos = 0;
  for (const query of queries) {
    const queryStart = content.indexOf(query, currentPos);
    const queryEnd = queryStart + query.length;
    if (cursorPosition >= queryStart && cursorPosition <= queryEnd)
      return { query, start: queryStart, end: queryEnd };
    currentPos = queryEnd + 1;
  }
  if (queries.length > 0) {
    const lastQuery = queries[queries.length - 1];
    const lastQueryStart = content.lastIndexOf(lastQuery);
    return { query: lastQuery, start: lastQueryStart, end: lastQueryStart + lastQuery.length };
  }
  return { query: '', start: 0, end: 0 };
};

export const formatQuery = (query: string): string => {
  if (!query) return '';
  const lines = query.split('\n');
  const formattedLines: string[] = [];
  let indentLevel = 0;
  const indentSize = 2;
  const keywords = ['MATCH', 'WHERE', 'RETURN', 'CREATE', 'DELETE', 'SET', 'REMOVE', 'WITH', 'UNWIND', 'CALL', 'YIELD', 'ORDER BY', 'LIMIT', 'SKIP', 'UNION'];
  for (const line of lines) {
    const trimmedLine = line.trim();
    if (!trimmedLine) { formattedLines.push(''); continue; }
    if (trimmedLine.startsWith('}')) indentLevel = Math.max(0, indentLevel - 1);
    const startsWithKeyword = keywords.some(kw => trimmedLine.toUpperCase().startsWith(kw) || trimmedLine.toUpperCase().startsWith(kw + ' '));
    let indent = ' '.repeat(indentLevel * indentSize);
    if (startsWithKeyword && indentLevel > 0) indent = ' '.repeat(Math.max(0, (indentLevel - 1) * indentSize));
    formattedLines.push(indent + trimmedLine);
    if (trimmedLine.endsWith('{')) indentLevel++;
  }
  return formattedLines.join('\n');
};

export const toggleComment = (line: string): string => {
  const trimmed = line.trim();
  if (trimmed.startsWith('//')) return line.replace(/\/\/\s?/, '');
  const leadingWhitespace = line.match(/^\s*/)?.[0] || '';
  return leadingWhitespace + '// ' + trimmed;
};

export const validateQuery = (query: string): { valid: boolean; error?: string } => {
  if (!query || !query.trim()) return { valid: false, error: 'Query is empty' };
  const trimmed = query.trim();
  const validStartKeywords = ['MATCH', 'CREATE', 'MERGE', 'DELETE', 'REMOVE', 'SET', 'RETURN', 'WITH', 'UNWIND', 'CALL', 'LOAD', 'FOREACH', 'START', 'PROFILE', 'EXPLAIN'];
  const firstWord = trimmed.split(/\s+/)[0].toUpperCase();
  if (!validStartKeywords.includes(firstWord)) return { valid: false, error: 'Query must start with a valid Cypher keyword' };
  let parenCount = 0, braceCount = 0, bracketCount = 0;
  let inString = false, stringChar = '';
  for (const char of trimmed) {
    if (!inString && (char === '"' || char === "'")) { inString = true; stringChar = char; continue; }
    if (inString && char === stringChar) { inString = false; continue; }
    if (inString) continue;
    if (char === '(') parenCount++; if (char === ')') parenCount--;
    if (char === '{') braceCount++; if (char === '}') braceCount--;
    if (char === '[') bracketCount++; if (char === ']') bracketCount--;
  }
  if (parenCount !== 0) return { valid: false, error: 'Unbalanced parentheses' };
  if (braceCount !== 0) return { valid: false, error: 'Unbalanced braces' };
  if (bracketCount !== 0) return { valid: false, error: 'Unbalanced brackets' };
  return { valid: true };
};

export const extractQueryInfo = (query: string): { type: string; entities: string[] } => {
  const upperQuery = query.toUpperCase();
  const entities: string[] = [];
  let type = 'UNKNOWN';
  if (upperQuery.includes('MATCH')) type = 'READ';
  if (upperQuery.includes('CREATE') || upperQuery.includes('MERGE')) type = 'WRITE';
  if (upperQuery.includes('DELETE') || upperQuery.includes('REMOVE')) type = 'DELETE';
  if (upperQuery.includes('SET')) type = 'UPDATE';
  const labelMatches = query.match(/:\s*([A-Za-z][A-Za-z0-9_]*)/g);
  if (labelMatches) labelMatches.forEach(m => { const l = m.replace(/^:\s*/, ''); if (!entities.includes(l)) entities.push(l); });
  return { type, entities };
};
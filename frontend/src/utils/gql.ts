/**
 * Cypher query utility functions
 */

// Split multiple queries separated by semicolons
export const splitQueries = (content: string): string[] => {
  if (!content || !content.trim()) {
    return [];
  }

  const queries: string[] = [];
  let currentQuery = '';
  let inString = false;
  let stringChar = '';
  let escaped = false;

  for (let i = 0; i < content.length; i++) {
    const char = content[i];

    if (escaped) {
      currentQuery += char;
      escaped = false;
      continue;
    }

    if (char === '\\') {
      currentQuery += char;
      escaped = true;
      continue;
    }

    if (!inString && (char === '"' || char === "'" || char === '`')) {
      inString = true;
      stringChar = char;
      currentQuery += char;
      continue;
    }

    if (inString && char === stringChar) {
      inString = false;
      stringChar = '';
      currentQuery += char;
      continue;
    }

    if (!inString && char === ';') {
      const trimmed = currentQuery.trim();
      if (trimmed) {
        queries.push(trimmed);
      }
      currentQuery = '';
      continue;
    }

    currentQuery += char;
  }

  // Add the last query
  const trimmed = currentQuery.trim();
  if (trimmed) {
    queries.push(trimmed);
  }

  return queries;
};

// Get the query at cursor position
export const getQueryAtCursor = (content: string, cursorPosition: number): { query: string; start: number; end: number } => {
  if (!content) {
    return { query: '', start: 0, end: 0 };
  }

  const queries = splitQueries(content);
  let currentPos = 0;

  for (const query of queries) {
    const queryStart = content.indexOf(query, currentPos);
    const queryEnd = queryStart + query.length;

    if (cursorPosition >= queryStart && cursorPosition <= queryEnd) {
      return { query, start: queryStart, end: queryEnd };
    }

    currentPos = queryEnd + 1; // +1 for the semicolon
  }

  // If cursor is at the end, return the last query
  if (queries.length > 0) {
    const lastQuery = queries[queries.length - 1];
    const lastQueryStart = content.lastIndexOf(lastQuery);
    return { query: lastQuery, start: lastQueryStart, end: lastQueryStart + lastQuery.length };
  }

  return { query: '', start: 0, end: 0 };
};

// Format Cypher query with basic indentation
export const formatQuery = (query: string): string => {
  if (!query) return '';

  const lines = query.split('\n');
  const formattedLines: string[] = [];
  let indentLevel = 0;
  const indentSize = 2;

  const keywords = ['MATCH', 'WHERE', 'RETURN', 'CREATE', 'DELETE', 'SET', 'REMOVE', 'WITH', 'UNWIND', 'CALL', 'YIELD', 'ORDER BY', 'LIMIT', 'SKIP', 'UNION'];

  for (const line of lines) {
    const trimmedLine = line.trim();
    
    if (!trimmedLine) {
      formattedLines.push('');
      continue;
    }

    // Decrease indent for closing braces
    if (trimmedLine.startsWith('}')) {
      indentLevel = Math.max(0, indentLevel - 1);
    }

    // Check if line starts with a keyword
    const startsWithKeyword = keywords.some(kw => 
      trimmedLine.toUpperCase().startsWith(kw) || 
      trimmedLine.toUpperCase().startsWith(kw + ' ')
    );

    // Add appropriate indentation
    let indent = ' '.repeat(indentLevel * indentSize);
    if (startsWithKeyword && indentLevel > 0) {
      // Keywords at base level of current block
      indent = ' '.repeat(Math.max(0, (indentLevel - 1) * indentSize));
    }

    formattedLines.push(indent + trimmedLine);

    // Increase indent for opening braces
    if (trimmedLine.endsWith('{')) {
      indentLevel++;
    }
  }

  return formattedLines.join('\n');
};

// Toggle comment for a line
export const toggleComment = (line: string): string => {
  const trimmed = line.trim();
  if (trimmed.startsWith('//')) {
    return line.replace(/\/\/\s?/, '');
  } else {
    const leadingWhitespace = line.match(/^\s*/)?.[0] || '';
    return leadingWhitespace + '// ' + trimmed;
  }
};

// Validate basic Cypher syntax
export const validateQuery = (query: string): { valid: boolean; error?: string } => {
  if (!query || !query.trim()) {
    return { valid: false, error: 'Query is empty' };
  }

  const trimmed = query.trim();
  
  // Check for basic Cypher keywords
  const validStartKeywords = ['MATCH', 'CREATE', 'MERGE', 'DELETE', 'REMOVE', 'SET', 'RETURN', 'WITH', 'UNWIND', 'CALL', 'LOAD', 'FOREACH', 'START', 'PROFILE', 'EXPLAIN'];
  
  const firstWord = trimmed.split(/\s+/)[0].toUpperCase();
  
  if (!validStartKeywords.includes(firstWord)) {
    return { valid: false, error: `Query must start with a valid Cypher keyword (e.g., MATCH, CREATE, RETURN)` };
  }

  // Check for balanced parentheses
  let parenCount = 0;
  let braceCount = 0;
  let bracketCount = 0;
  let inString = false;
  let stringChar = '';

  for (const char of trimmed) {
    if (!inString && (char === '"' || char === "'")) {
      inString = true;
      stringChar = char;
      continue;
    }
    if (inString && char === stringChar) {
      inString = false;
      continue;
    }
    if (inString) continue;

    if (char === '(') parenCount++;
    if (char === ')') parenCount--;
    if (char === '{') braceCount++;
    if (char === '}') braceCount--;
    if (char === '[') bracketCount++;
    if (char === ']') bracketCount--;
  }

  if (parenCount !== 0) return { valid: false, error: 'Unbalanced parentheses' };
  if (braceCount !== 0) return { valid: false, error: 'Unbalanced braces' };
  if (bracketCount !== 0) return { valid: false, error: 'Unbalanced brackets' };

  return { valid: true };
};

// Extract query metadata
export const extractQueryInfo = (query: string): { type: string; entities: string[] } => {
  const upperQuery = query.toUpperCase();
  const entities: string[] = [];
  
  // Determine query type
  let type = 'UNKNOWN';
  if (upperQuery.includes('MATCH')) type = 'READ';
  if (upperQuery.includes('CREATE') || upperQuery.includes('MERGE')) type = 'WRITE';
  if (upperQuery.includes('DELETE') || upperQuery.includes('REMOVE')) type = 'DELETE';
  if (upperQuery.includes('SET')) type = 'UPDATE';
  
  // Extract node labels (basic regex)
  const labelMatches = query.match(/:\s*([A-Za-z][A-Za-z0-9_]*)/g);
  if (labelMatches) {
    labelMatches.forEach(match => {
      const label = match.replace(/^:\s*/, '');
      if (!entities.includes(label)) {
        entities.push(label);
      }
    });
  }

  // Extract relationship types
  const relMatches = query.match(/:\s*\[?\s*:?\s*([A-Z][A-Z_0-9]*)\s*\]?/gu);
  if (relMatches) {
    relMatches.forEach(match => {
      // eslint-disable-next-line no-useless-escape
      const rel = match.replace(/[:\[\]\s]/gu, '');
      if (rel && !entities.includes(rel)) {
        entities.push(rel);
      }
    });
  }

  return { type, entities };
};

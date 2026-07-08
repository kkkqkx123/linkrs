import React from 'react';
import { Empty } from 'antd';
import type { QueryResult } from '@/types/query';
import styles from './index.module.less';

interface ResultJsonProps {
  result: QueryResult;
}

const ResultJson: React.FC<ResultJsonProps> = ({ result }) => {
  if (!result) {
    return (
      <Empty
        description="No data available"
        className={styles.empty}
      />
    );
  }

  // Convert result to object format
  const jsonData = {
    columns: result.columns,
    rows: result.rows,
    rowCount: result.rowCount,
  };

  const jsonString = JSON.stringify(jsonData, null, 2);

  // Simple syntax highlighting
  const highlightJson = (json: string): React.ReactNode[] => {
    const lines = json.split('\n');
    return lines.map((line, index) => {
      // Highlight keys
      const highlightedLine = line
        .replace(/"([^"]+)":/g, '<span class="styles.key">"$1"</span>:')
        .replace(/: "([^"]*)"/g, ': <span class="styles.string">"$1"</span>')
        .replace(/: (\d+)/g, ': <span class="styles.number">$1</span>')
        .replace(/: (true|false)/g, ': <span class="styles.boolean">$1</span>')
        .replace(/: (null)/g, ': <span class="styles.null">$1</span>');

      return (
        <div key={index} className={styles.line}>
          <span className={styles.lineNumber}>{index + 1}</span>
          <span 
            className={styles.lineContent}
            dangerouslySetInnerHTML={{ __html: highlightedLine }}
          />
        </div>
      );
    });
  };

  return (
    <div className={styles.resultJson}>
      <pre className={styles.pre}>
        <code className={styles.code}>
          {highlightJson(jsonString)}
        </code>
      </pre>
    </div>
  );
};

export default ResultJson;

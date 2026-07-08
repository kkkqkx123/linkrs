import React, { useRef, useEffect } from 'react';
import { Input, Button, Space, Tooltip, Typography } from 'antd';
import {
  PlayCircleOutlined,
  ClearOutlined,
  StarOutlined,
  HistoryOutlined,
  BookOutlined,
} from '@ant-design/icons';
import { useConsoleStore } from '@/stores/console';
import { toggleComment } from '@/utils/gql';
import styles from './index.module.less';

const { TextArea } = Input;
const { Text } = Typography;

interface QueryEditorProps {
  onOpenHistory: () => void;
  onOpenFavorites: () => void;
  onSaveFavorite: () => void;
}

const QueryEditor: React.FC<QueryEditorProps> = ({
  onOpenHistory,
  onOpenFavorites,
  onSaveFavorite,
}) => {
  const textareaRef = useRef<HTMLTextAreaElement>(null);
  const {
    editorContent,
    isExecuting,
    cursorPosition,
    setEditorContent,
    setCursorPosition,
    executeQuery,
  } = useConsoleStore();

  // Handle tab indentation
  const handleTab = (shiftKey: boolean) => {
    const textarea = textareaRef.current;
    if (!textarea) return;

    const start = textarea.selectionStart;
    const end = textarea.selectionEnd;
    const value = textarea.value;

    if (shiftKey) {
      // Shift+Tab: remove indentation
      const lineStart = value.lastIndexOf('\n', start - 1) + 1;
      const lineContent = value.substring(lineStart, end);
      const newLineContent = lineContent.replace(/^ {2}/, '');
      const newValue = value.substring(0, lineStart) + newLineContent + value.substring(end);
      setEditorContent(newValue);
      
      setTimeout(() => {
        textarea.selectionStart = start - (lineContent.length - newLineContent.length);
        textarea.selectionEnd = end - (lineContent.length - newLineContent.length);
      }, 0);
    } else {
      // Tab: add indentation
      const newValue = value.substring(0, start) + '  ' + value.substring(end);
      setEditorContent(newValue);
      
      setTimeout(() => {
        textarea.selectionStart = textarea.selectionEnd = start + 2;
      }, 0);
    }
  };

  // Toggle comment for current line
  const handleToggleComment = () => {
    const textarea = textareaRef.current;
    if (!textarea) return;

    const start = textarea.selectionStart;
    const value = textarea.value;
    
    // Find the current line
    const lineStart = value.lastIndexOf('\n', start - 1) + 1;
    const lineEnd = value.indexOf('\n', start);
    const actualLineEnd = lineEnd === -1 ? value.length : lineEnd;
    
    const line = value.substring(lineStart, actualLineEnd);
    const newLine = toggleComment(line);
    
    const newValue = value.substring(0, lineStart) + newLine + value.substring(actualLineEnd);
    setEditorContent(newValue);
  };

  // Handle keyboard shortcuts
  const handleKeyDown = (e: React.KeyboardEvent<HTMLTextAreaElement>) => {
    // Ctrl/Cmd + Enter to execute
    if ((e.ctrlKey || e.metaKey) && e.key === 'Enter') {
      e.preventDefault();
      executeQuery();
      return;
    }

    // Shift + Enter to execute
    if (e.shiftKey && e.key === 'Enter') {
      e.preventDefault();
      executeQuery();
      return;
    }

    // Ctrl/Cmd + / to toggle comment
    if ((e.ctrlKey || e.metaKey) && e.key === '/') {
      e.preventDefault();
      handleToggleComment();
      return;
    }

    // Tab to insert indentation
    if (e.key === 'Tab') {
      e.preventDefault();
      handleTab(e.shiftKey);
    }
  };

  // Update cursor position
  const handleCursorChange = () => {
    const textarea = textareaRef.current;
    if (!textarea) return;

    const position = textarea.selectionStart;
    const textBeforeCursor = textarea.value.substring(0, position);
    const lines = textBeforeCursor.split('\n');
    const line = lines.length;
    const column = lines[lines.length - 1].length + 1;
    
    setCursorPosition(line, column);
  };

  // Clear editor content
  const handleClear = () => {
    setEditorContent('');
    textareaRef.current?.focus();
  };

  // Auto-resize textarea
  useEffect(() => {
    const textarea = textareaRef.current;
    if (textarea) {
      textarea.style.height = 'auto';
      textarea.style.height = `${Math.max(150, Math.min(400, textarea.scrollHeight))}px`;
    }
  }, [editorContent]);

  return (
    <div className={styles.queryEditor}>
      {/* Toolbar */}
      <div className={styles.toolbar}>
        <Space>
          <Tooltip title="Execute Query (Ctrl+Enter)">
            <Button
              type="primary"
              icon={<PlayCircleOutlined />}
              onClick={executeQuery}
              loading={isExecuting}
              disabled={!editorContent.trim()}
            >
              Execute
            </Button>
          </Tooltip>
          
          <Tooltip title="Clear Editor">
            <Button
              icon={<ClearOutlined />}
              onClick={handleClear}
              disabled={!editorContent.trim()}
            >
              Clear
            </Button>
          </Tooltip>
          
          <Tooltip title="Save to Favorites">
            <Button
              icon={<StarOutlined />}
              onClick={onSaveFavorite}
              disabled={!editorContent.trim()}
            >
              Save
            </Button>
          </Tooltip>
        </Space>

        <Space>
          <Tooltip title="Query History">
            <Button
              icon={<HistoryOutlined />}
              onClick={onOpenHistory}
            >
              History
            </Button>
          </Tooltip>
          
          <Tooltip title="Favorites">
            <Button
              icon={<BookOutlined />}
              onClick={onOpenFavorites}
            >
              Favorites
            </Button>
          </Tooltip>
        </Space>
      </div>

      {/* Editor */}
      <div className={styles.editorContainer}>
        <TextArea
          value={editorContent}
          onChange={(e) => setEditorContent(e.target.value)}
          onKeyDown={handleKeyDown}
          onClick={handleCursorChange}
          onKeyUp={handleCursorChange}
          placeholder="Enter your Cypher query here...&#10;Example: MATCH (n) RETURN n LIMIT 10"
          className={styles.textarea}
          spellCheck={false}
        />
      </div>

      {/* Status Bar */}
      <div className={styles.statusBar}>
        <Text type="secondary" className={styles.statusText}>
          Line {cursorPosition.line}, Column {cursorPosition.column}
        </Text>
        <Text type="secondary" className={styles.statusText}>
          {editorContent.length} characters
        </Text>
      </div>
    </div>
  );
};

export default QueryEditor;

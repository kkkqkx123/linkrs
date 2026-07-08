import React from 'react';
import {
  Card,
  Space,
  Button,
  Segmented,
  Typography,
  Alert,
  Empty,
  Dropdown,
  Tooltip,
} from 'antd';
import {
  TableOutlined,
  CodeOutlined,
  DownloadOutlined,
  FileTextOutlined,
  FileExcelOutlined,
  ShareAltOutlined,
} from '@ant-design/icons';
import type { QueryResult, QueryError } from '@/types/query';
import ResultTable from '../ResultTable';
import ResultJson from '../ResultJson';
import GraphView from '../GraphView';
import { exportToCSV, exportToJSON } from '@/utils/export';
import { formatExecutionTime, formatRowCount } from '@/utils/parseData';
import styles from './index.module.less';

const { Text } = Typography;

interface OutputBoxProps {
  result: QueryResult | null;
  error: QueryError | null;
  executionTime: number;
  activeView: 'table' | 'json' | 'graph';
  onViewChange: (view: 'table' | 'json' | 'graph') => void;
}

const OutputBox: React.FC<OutputBoxProps> = ({
  result,
  error,
  executionTime,
  activeView,
  onViewChange,
}) => {
  // Handle export
  const handleExport = (format: 'csv' | 'json') => {
    if (!result) return;

    const timestamp = new Date().toISOString().replace(/[:.]/g, '-');
    const filename = `query_result_${timestamp}`;

    if (format === 'csv') {
      exportToCSV(result, `${filename}.csv`);
    } else {
      exportToJSON(result, `${filename}.json`);
    }
  };

  // Export menu items
  const exportMenuItems = [
    {
      key: 'csv',
      icon: <FileExcelOutlined />,
      label: 'Export to CSV',
      onClick: () => handleExport('csv'),
    },
    {
      key: 'json',
      icon: <FileTextOutlined />,
      label: 'Export to JSON',
      onClick: () => handleExport('json'),
    },
  ];

  // Render content based on state
  const renderContent = () => {
    // Show error if present
    if (error) {
      return (
        <Alert
          message="Query Error"
          description={
            <div>
              <Text strong>{error.code}</Text>
              <div>{error.message}</div>
              {error.position && (
                <Text type="secondary">
                  at Line {error.position.line}, Column {error.position.column}
                </Text>
              )}
            </div>
          }
          type="error"
          showIcon
          className={styles.error}
        />
      );
    }

    // Show empty state if no result
    if (!result) {
      return (
        <Empty
          description="Execute a query to see results"
          className={styles.empty}
        />
      );
    }

    // Show result based on active view
    return (
      <div className={styles.resultContainer}>
        {activeView === 'table' ? (
          <ResultTable result={result} />
        ) : activeView === 'json' ? (
          <ResultJson result={result} />
        ) : (
          <GraphView data={result.data} />
        )}
      </div>
    );
  };

  const hasResult = result && result.rowCount > 0;

  return (
    <Card
      className={styles.outputBox}
      title={
        <div className={styles.header}>
          <Space>
            <Text strong>Results</Text>
            {result && (
              <Text type="secondary" className={styles.stats}>
                {formatRowCount(result.rowCount)} • {formatExecutionTime(executionTime)}
              </Text>
            )}
          </Space>

          <Space>
            <Segmented
              value={activeView}
              onChange={(value) => onViewChange(value as 'table' | 'json' | 'graph')}
              options={[
                {
                  value: 'table',
                  icon: <TableOutlined />,
                  label: 'Table',
                },
                {
                  value: 'json',
                  icon: <CodeOutlined />,
                  label: 'JSON',
                },
                {
                  value: 'graph',
                  icon: <ShareAltOutlined />,
                  label: 'Graph',
                },
              ]}
            />

            <Dropdown
              menu={{ items: exportMenuItems }}
              placement="bottomRight"
              disabled={!hasResult}
            >
              <Tooltip title="Export Results">
                <Button
                  icon={<DownloadOutlined />}
                  disabled={!hasResult}
                >
                  Export
                </Button>
              </Tooltip>
            </Dropdown>
          </Space>
        </div>
      }
    >
      {renderContent()}
    </Card>
  );
};

export default OutputBox;

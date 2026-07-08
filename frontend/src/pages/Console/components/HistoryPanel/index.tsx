import React from 'react';
import {
  Drawer,
  List,
  Button,
  Typography,
  Empty,
  Popconfirm,
  Tag,
  Tooltip,
} from 'antd';
import {
  HistoryOutlined,
  DeleteOutlined,
  ClockCircleOutlined,
  CheckCircleOutlined,
  CloseCircleOutlined,
} from '@ant-design/icons';
import { useConsoleStore, type QueryHistoryItem } from '@/stores/console';
import { formatExecutionTime, formatRowCount } from '@/utils/parseData';
import styles from './index.module.less';

const { Text, Paragraph } = Typography;

interface HistoryPanelProps {
  open: boolean;
  onClose: () => void;
}

const HistoryPanel: React.FC<HistoryPanelProps> = ({ open, onClose }) => {
  const { history, clearHistory, loadFromHistory } = useConsoleStore();

  // Format timestamp
  const formatTime = (timestamp: number): string => {
    const date = new Date(timestamp);
    const now = new Date();
    const diff = now.getTime() - date.getTime();

    // Less than 1 minute
    if (diff < 60000) {
      return 'Just now';
    }

    // Less than 1 hour
    if (diff < 3600000) {
      const minutes = Math.floor(diff / 60000);
      return `${minutes} min ago`;
    }

    // Less than 24 hours
    if (diff < 86400000) {
      const hours = Math.floor(diff / 3600000);
      return `${hours} hour${hours > 1 ? 's' : ''} ago`;
    }

    // Format date
    return date.toLocaleDateString();
  };

  // Handle load from history
  const handleLoad = (query: string) => {
    loadFromHistory(query);
    onClose();
  };

  // Render history item
  const renderItem = (item: QueryHistoryItem) => (
    <List.Item
      className={styles.historyItem}
      actions={[
        <Tooltip title="Load to Editor" key="load">
          <Button
            type="link"
            size="small"
            onClick={() => handleLoad(item.query)}
          >
            Load
          </Button>
        </Tooltip>,
      ]}
    >
      <div className={styles.itemContent}>
        <div className={styles.queryText}>
          <Paragraph
            ellipsis={{ rows: 2 }}
            className={styles.queryParagraph}
          >
            {item.query}
          </Paragraph>
        </div>
        <div className={styles.itemMeta}>
          <Space size="small">
            {item.success ? (
              <Tag icon={<CheckCircleOutlined />} color="success">
                Success
              </Tag>
            ) : (
              <Tag icon={<CloseCircleOutlined />} color="error">
                Failed
              </Tag>
            )}
            <Text type="secondary" className={styles.metaText}>
              <ClockCircleOutlined /> {formatTime(item.timestamp)}
            </Text>
            {item.success && (
              <>
                <Text type="secondary" className={styles.metaText}>
                  {formatRowCount(item.rowCount)}
                </Text>
                <Text type="secondary" className={styles.metaText}>
                  {formatExecutionTime(item.executionTime)}
                </Text>
              </>
            )}
          </Space>
        </div>
      </div>
    </List.Item>
  );

  return (
    <Drawer
      title={
        <div className={styles.drawerTitle}>
          <HistoryOutlined />
          <span>Query History</span>
          <Tag className={styles.countTag}>{history.length}</Tag>
        </div>
      }
      placement="right"
      onClose={onClose}
      open={open}
      width={400}
      footer={
        history.length > 0 && (
          <div className={styles.drawerFooter}>
            <Popconfirm
              title="Clear History"
              description="Are you sure you want to clear all history?"
              onConfirm={clearHistory}
              okText="Yes"
              cancelText="No"
            >
              <Button
                danger
                icon={<DeleteOutlined />}
                block
              >
                Clear History
              </Button>
            </Popconfirm>
          </div>
        )
      }
    >
      {history.length === 0 ? (
        <Empty
          description="No query history yet"
          className={styles.empty}
        />
      ) : (
        <List
          className={styles.historyList}
          dataSource={history}
          renderItem={renderItem}
          split={false}
        />
      )}
    </Drawer>
  );
};

// Need to import Space
import { Space } from 'antd';

export default HistoryPanel;

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
  BookOutlined,
  DeleteOutlined,
  PlayCircleOutlined,
  EditOutlined,
  StarOutlined,
} from '@ant-design/icons';
import { useConsoleStore, type QueryFavoriteItem } from '@/stores/console';
import styles from './index.module.less';

const { Text, Paragraph } = Typography;

interface FavoritePanelProps {
  open: boolean;
  onClose: () => void;
  onSaveNew: () => void;
}

const FavoritePanel: React.FC<FavoritePanelProps> = ({
  open,
  onClose,
  onSaveNew,
}) => {
  const {
    favorites,
    removeFromFavorites,
    loadFromFavorites,
    executeQueryByText,
  } = useConsoleStore();

  // Handle load favorite to editor
  const handleLoad = (query: string) => {
    loadFromFavorites(query);
    onClose();
  };

  // Handle execute favorite directly
  const handleExecute = async (query: string) => {
    onClose();
    await executeQueryByText(query);
  };

  // Format date
  const formatDate = (timestamp: number): string => {
    const date = new Date(timestamp);
    return date.toLocaleDateString(undefined, {
      year: 'numeric',
      month: 'short',
      day: 'numeric',
    });
  };

  // Render favorite item
  const renderItem = (item: QueryFavoriteItem) => (
    <List.Item
      className={styles.favoriteItem}
      actions={[
        <Tooltip title="Execute" key="execute">
          <Button
            type="link"
            size="small"
            icon={<PlayCircleOutlined />}
            onClick={() => handleExecute(item.query)}
          />
        </Tooltip>,
        <Tooltip title="Load to Editor" key="load">
          <Button
            type="link"
            size="small"
            icon={<EditOutlined />}
            onClick={() => handleLoad(item.query)}
          />
        </Tooltip>,
        <Popconfirm
          title="Delete Favorite"
          description={`Are you sure you want to delete "${item.name}"?`}
          onConfirm={() => removeFromFavorites(item.id)}
          okText="Yes"
          cancelText="No"
          key="delete"
        >
          <Button
            type="link"
            danger
            size="small"
            icon={<DeleteOutlined />}
          />
        </Popconfirm>,
      ]}
    >
      <div className={styles.itemContent}>
        <div className={styles.itemHeader}>
          <Text strong className={styles.itemName}>
            {item.name}
          </Text>
          <Text type="secondary" className={styles.itemDate}>
            {formatDate(item.createdAt)}
          </Text>
        </div>
        <div className={styles.queryText}>
          <Paragraph
            ellipsis={{ rows: 2 }}
            className={styles.queryParagraph}
          >
            {item.query}
          </Paragraph>
        </div>
      </div>
    </List.Item>
  );

  return (
    <Drawer
      title={
        <div className={styles.drawerTitle}>
          <BookOutlined />
          <span>Favorites</span>
          <Tag className={styles.countTag}>{favorites.length}/30</Tag>
        </div>
      }
      placement="right"
      onClose={onClose}
      open={open}
      width={400}
      footer={
        <div className={styles.drawerFooter}>
          <Button
            type="primary"
            icon={<StarOutlined />}
            onClick={onSaveNew}
            block
          >
            Save Current Query
          </Button>
        </div>
      }
    >
      {favorites.length === 0 ? (
        <Empty
          description="No favorites yet"
          className={styles.empty}
        />
      ) : (
        <List
          className={styles.favoriteList}
          dataSource={favorites}
          renderItem={renderItem}
          split={false}
        />
      )}
    </Drawer>
  );
};

export default FavoritePanel;

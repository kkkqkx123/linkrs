import React, { useEffect, useState, useMemo } from 'react';
import {
  Card,
  Table,
  Button,
  Space as AntSpace,
  Tooltip,
  Popconfirm,
  message,
  Empty,
  Spin,
  Typography,
  Tag,
  Input,
} from 'antd';
import {
  PlusOutlined,
  ReloadOutlined,
  EyeOutlined,
  DeleteOutlined,
  DatabaseOutlined,
  FileTextOutlined,
  SearchOutlined,
  ClearOutlined,
} from '@ant-design/icons';
import { useSchemaStore } from '@/stores/schema';
import type { Space } from '@/types/schema';
import SpaceCreateModal from '../components/SpaceCreateModal';
import SpaceDetailModal from '../components/SpaceDetailModal';
import DDLExportModal from './components/DDLExportModal';
import styles from './index.module.less';

const { Title, Text } = Typography;

const EmptyState: React.FC<{ onCreate: () => void }> = ({ onCreate }) => (
  <Empty
    image={Empty.PRESENTED_IMAGE_SIMPLE}
    description={
      <div>
        <p>No Spaces found</p>
        <Text type="secondary">
          Create a new Space to start organizing your graph data
        </Text>
      </div>
    }
  >
    <Button type="primary" icon={<PlusOutlined />} onClick={onCreate}>
      Create Space
    </Button>
  </Empty>
);

const SpaceList: React.FC = () => {
  const {
    spaces,
    isLoadingSpaces,
    spacesError,
    currentSpace,
    setCurrentSpace,
    fetchSpaces,
    deleteSpace,
  } = useSchemaStore();

  const [createModalVisible, setCreateModalVisible] = useState(false);
  const [detailModalVisible, setDetailModalVisible] = useState(false);
  const [ddlModalVisible, setDDLModalVisible] = useState(false);
  const [selectedSpace, setSelectedSpace] = useState<Space | null>(null);

  // Search and pagination state
  const [searchText, setSearchText] = useState('');
  const [currentPage, setCurrentPage] = useState(1);
  const [pageSize, setPageSize] = useState(10);

  // Filtered spaces based on search
  const filteredSpaces = useMemo(() => {
    if (!searchText.trim()) return spaces;
    return spaces.filter((space) =>
      space.name.toLowerCase().includes(searchText.toLowerCase())
    );
  }, [spaces, searchText]);

  // Paginated spaces
  const paginatedSpaces = useMemo(() => {
    const start = (currentPage - 1) * pageSize;
    return filteredSpaces.slice(start, start + pageSize);
  }, [filteredSpaces, currentPage, pageSize]);

  useEffect(() => {
    fetchSpaces();
  }, [fetchSpaces]);

  const handleRefresh = () => {
    fetchSpaces();
    setSearchText('');
    setCurrentPage(1);
    message.success('Space list refreshed');
  };

  const handleSearchChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    setSearchText(e.target.value);
    setCurrentPage(1);
  };

  const handleClearSearch = () => {
    setSearchText('');
    setCurrentPage(1);
  };

  const handleCreateSuccess = () => {
    setCreateModalVisible(false);
    fetchSpaces();
  };

  const handleDelete = async (space: Space) => {
    try {
      await deleteSpace(space.name);
      message.success(`Space "${space.name}" deleted successfully`);
    } catch (err: unknown) {
      const errorMessage = err instanceof Error ? err.message : 'Failed to delete space';
      message.error(errorMessage);
    }
  };

  const handleViewDetail = (space: Space) => {
    setSelectedSpace(space);
    setDetailModalVisible(true);
  };

  const handleCloseDetail = () => {
    setDetailModalVisible(false);
    setSelectedSpace(null);
  };

  const handleShowDDL = (space: Space) => {
    setSelectedSpace(space);
    setDDLModalVisible(true);
  };

  const handleCloseDDL = () => {
    setDDLModalVisible(false);
    setSelectedSpace(null);
  };

  const handleRowClick = (record: Space) => {
    setCurrentSpace(record.name);
    message.info(`Switched to Space: ${record.name}`);
  };

  const columns = [
    {
      title: 'Name',
      dataIndex: 'name',
      key: 'name',
      render: (name: string) => (
        <AntSpace>
          <DatabaseOutlined />
          <Text strong={currentSpace === name}>{name}</Text>
          {currentSpace === name && <Tag color="blue">Current</Tag>}
        </AntSpace>
      ),
      sorter: (a: Space, b: Space) => a.name.localeCompare(b.name),
    },
    {
      title: 'Vid Type',
      dataIndex: 'vid_type',
      key: 'vid_type',
      render: (vidType: string) => <Tag>{vidType}</Tag>,
    },
    {
      title: 'Created At',
      dataIndex: 'created_at',
      key: 'created_at',
      render: (timestamp: number) => {
        if (!timestamp) return 'N/A';
        return new Date(timestamp * 1000).toLocaleString();
      },
    },
    {
      title: 'Actions',
      key: 'actions',
      width: 200,
      render: (_: unknown, record: Space) => (
        <AntSpace size="small">
          <Tooltip title="View Details">
            <Button
              icon={<EyeOutlined />}
              size="small"
              onClick={(e) => {
                e.stopPropagation();
                handleViewDetail(record);
              }}
            />
          </Tooltip>
          <Tooltip title="Export DDL">
            <Button
              icon={<FileTextOutlined />}
              size="small"
              onClick={(e) => {
                e.stopPropagation();
                handleShowDDL(record);
              }}
            />
          </Tooltip>
          <Tooltip title="Delete Space">
            <Popconfirm
              title="Delete Space"
              description={
                <div>
                  <p>Are you sure you want to delete &quot;{record.name}&quot;?</p>
                  <Text type="danger">
                    This action cannot be undone and all data will be lost.
                  </Text>
                </div>
              }
              onConfirm={(e) => {
                e?.stopPropagation();
                handleDelete(record);
              }}
              onCancel={(e) => e?.stopPropagation()}
              okText="Delete"
              okType="danger"
              cancelText="Cancel"
            >
              <Button
                icon={<DeleteOutlined />}
                size="small"
                danger
                onClick={(e) => e.stopPropagation()}
              />
            </Popconfirm>
          </Tooltip>
        </AntSpace>
      ),
    },
  ];



  return (
    <div className={styles.container}>
      <Card
        title={
          <div className={styles.header}>
            <Title level={4} className={styles.title}>
              Space Management
            </Title>
            <AntSpace>
              <Input
                placeholder="Search spaces..."
                prefix={<SearchOutlined />}
                suffix={
                  searchText ? (
                    <ClearOutlined
                      onClick={handleClearSearch}
                      style={{ cursor: 'pointer' }}
                    />
                  ) : null
                }
                value={searchText}
                onChange={handleSearchChange}
                style={{ width: 200 }}
                allowClear
              />
              <Tooltip title="Refresh">
                <Button
                  icon={<ReloadOutlined />}
                  onClick={handleRefresh}
                  loading={isLoadingSpaces}
                />
              </Tooltip>
              <Button
                type="primary"
                icon={<PlusOutlined />}
                onClick={() => setCreateModalVisible(true)}
              >
                Create Space
              </Button>
            </AntSpace>
          </div>
        }
        className={styles.card}
      >
        {spacesError ? (
          <Empty description={`Error: ${spacesError}`} />
        ) : spaces.length === 0 && !isLoadingSpaces ? (
          <EmptyState onCreate={() => setCreateModalVisible(true)} />
        ) : (
          <Spin spinning={isLoadingSpaces}>
            <Table
              columns={columns}
              dataSource={paginatedSpaces}
              rowKey="id"
              pagination={{
                current: currentPage,
                pageSize: pageSize,
                total: filteredSpaces.length,
                onChange: (page, size) => {
                  setCurrentPage(page);
                  if (size) setPageSize(size);
                },
                showSizeChanger: true,
                showTotal: (total) => `Total ${total} spaces`,
                pageSizeOptions: ['10', '20', '50', '100'],
              }}
              onRow={(record) => ({
                onClick: () => handleRowClick(record),
                className: currentSpace === record.name ? styles.currentRow : '',
              })}
              rowClassName={() => styles.row}
              locale={{
                emptyText: searchText ? (
                  <Empty description={`No spaces found matching "${searchText}"`} />
                ) : (
                  <EmptyState onCreate={() => setCreateModalVisible(true)} />
                ),
              }}
            />
          </Spin>
        )}
      </Card>

      <SpaceCreateModal
        visible={createModalVisible}
        onCancel={() => setCreateModalVisible(false)}
        onSuccess={handleCreateSuccess}
      />

      <SpaceDetailModal
        visible={detailModalVisible}
        space={selectedSpace}
        onClose={handleCloseDetail}
      />

      <DDLExportModal
        visible={ddlModalVisible}
        space={selectedSpace?.name || ''}
        onCancel={handleCloseDDL}
      />
    </div>
  );
};

export default SpaceList;

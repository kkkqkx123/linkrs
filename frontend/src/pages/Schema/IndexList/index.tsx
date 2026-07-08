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
  Modal,
  Badge,
} from 'antd';
import {
  PlusOutlined,
  ReloadOutlined,
  EyeOutlined,
  DeleteOutlined,
  SyncOutlined,
  FileSearchOutlined,
} from '@ant-design/icons';
import { useSchemaStore } from '@/stores/schema';
import CreateIndexModal from './components/CreateIndexModal';
import type { IndexInfo } from '@/types/schema';
import styles from './index.module.less';

const { Title, Text } = Typography;

type IndexStatus = 'creating' | 'finished' | 'failed' | 'rebuilding';

interface IndexWithStatus extends IndexInfo {
  status?: IndexStatus;
  progress?: number;
}

const IndexList: React.FC = () => {
  const {
    indexes,
    isLoadingIndexes,
    indexesError,
    currentSpace,
    fetchIndexes,
    deleteIndex,
    rebuildIndex,
  } = useSchemaStore();

  const [searchText, setSearchText] = useState('');
  const [currentPage, setCurrentPage] = useState(1);
  const [pageSize, setPageSize] = useState(10);
  const [createModalVisible, setCreateModalVisible] = useState(false);
  const [detailModalVisible, setDetailModalVisible] = useState(false);
  const [selectedIndex, setSelectedIndex] = useState<IndexWithStatus | null>(null);

  // Filtered and paginated indexes
  const filteredIndexes = useMemo(() => {
    if (!searchText.trim()) return indexes;
    return indexes.filter((index) =>
      index.name.toLowerCase().includes(searchText.toLowerCase())
    );
  }, [indexes, searchText]);

  const paginatedIndexes = useMemo(() => {
    const start = (currentPage - 1) * pageSize;
    return filteredIndexes.slice(start, start + pageSize);
  }, [filteredIndexes, currentPage, pageSize]);

  useEffect(() => {
    if (currentSpace) {
      fetchIndexes(currentSpace);
    }
  }, [currentSpace, fetchIndexes]);

  const handleRefresh = () => {
    if (currentSpace) {
      fetchIndexes(currentSpace);
      setSearchText('');
      setCurrentPage(1);
      message.success('Index list refreshed');
    }
  };

  const handleSearchChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    setSearchText(e.target.value);
    setCurrentPage(1);
  };

  const handleCreateSuccess = () => {
    setCreateModalVisible(false);
    if (currentSpace) {
      fetchIndexes(currentSpace);
    }
  };

  const handleDelete = async (index: IndexWithStatus) => {
    try {
      if (currentSpace) {
        await deleteIndex(currentSpace, index.name);
        message.success(`Index "${index.name}" deleted successfully`);
      }
    } catch (err: unknown) {
      const errorMessage = err instanceof Error ? err.message : 'Failed to delete index';
      message.error(errorMessage);
    }
  };

  const handleRebuild = async (index: IndexWithStatus) => {
    try {
      if (currentSpace) {
        await rebuildIndex(currentSpace, index.name);
        message.success(`Index "${index.name}" rebuild started`);
      }
    } catch (err: unknown) {
      const errorMessage = err instanceof Error ? err.message : 'Failed to rebuild index';
      message.error(errorMessage);
    }
  };

  const handleViewDetail = (index: IndexWithStatus) => {
    setSelectedIndex(index);
    setDetailModalVisible(true);
  };

  const getStatusBadge = (status?: IndexStatus) => {
    switch (status) {
      case 'finished':
        return <Badge status="success" text="Finished" />;
      case 'creating':
        return <Badge status="processing" text="Creating" />;
      case 'rebuilding':
        return <Badge status="warning" text="Rebuilding" />;
      case 'failed':
        return <Badge status="error" text="Failed" />;
      default:
        return <Badge status="default" text="Unknown" />;
    }
  };



  const columns = [
    {
      title: 'Name',
      dataIndex: 'name',
      key: 'name',
      render: (name: string) => (
        <AntSpace>
          <FileSearchOutlined />
          <Text strong>{name}</Text>
        </AntSpace>
      ),
      sorter: (a: IndexInfo, b: IndexInfo) => a.name.localeCompare(b.name),
    },
    {
      title: 'Type',
      dataIndex: 'entity_type',
      key: 'type',
      render: (type: string) => (
        <Tag color={type === 'TAG' ? 'blue' : 'green'}>{type}</Tag>
      ),
    },
    {
      title: 'Entity',
      dataIndex: 'entity_name',
      key: 'entity',
    },
    {
      title: 'Fields',
      dataIndex: 'fields',
      key: 'fields',
      render: (fields: string[]) => fields.join(', '),
    },
    {
      title: 'Status',
      key: 'status',
      render: (_: unknown, record: IndexWithStatus) => getStatusBadge(record.status),
    },
    {
      title: 'Created At',
      dataIndex: 'created_at',
      key: 'created_at',
      render: (timestamp: number) => new Date(timestamp * 1000).toLocaleString(),
    },
    {
      title: 'Actions',
      key: 'actions',
      render: (_: unknown, record: IndexWithStatus) => (
        <AntSpace>
          <Tooltip title="View Details">
            <Button
              type="text"
              icon={<EyeOutlined />}
              onClick={() => handleViewDetail(record)}
            />
          </Tooltip>
          <Tooltip title="Rebuild">
            <Button
              type="text"
              icon={<SyncOutlined />}
              onClick={() => handleRebuild(record)}
              disabled={record.status === 'creating' || record.status === 'rebuilding'}
            />
          </Tooltip>
          <Tooltip title="Delete">
            <Popconfirm
              title="Delete Index"
              description={`Are you sure you want to delete index "${record.name}"?`}
              onConfirm={() => handleDelete(record)}
              okText="Yes"
              cancelText="No"
            >
              <Button type="text" danger icon={<DeleteOutlined />} />
            </Popconfirm>
          </Tooltip>
        </AntSpace>
      ),
    },
  ];

  if (!currentSpace) {
    return (
      <Card>
        <Empty description="Please select a space first" />
      </Card>
    );
  }

  return (
    <div className={styles.container}>
      <Card
        title={
          <AntSpace>
            <Title level={4} style={{ margin: 0 }}>Indexes</Title>
            <Text type="secondary">({filteredIndexes.length})</Text>
          </AntSpace>
        }
        extra={
          <AntSpace>
            <Input.Search
              placeholder="Search indexes..."
              value={searchText}
              onChange={handleSearchChange}
              allowClear
              style={{ width: 200 }}
            />
            <Tooltip title="Refresh">
              <Button icon={<ReloadOutlined />} onClick={handleRefresh} />
            </Tooltip>
            <Button
              type="primary"
              icon={<PlusOutlined />}
              onClick={() => setCreateModalVisible(true)}
            >
              Create Index
            </Button>
          </AntSpace>
        }
      >
        <Spin spinning={isLoadingIndexes}>
          {indexesError ? (
            <Empty description={indexesError} />
          ) : (
            <Table
              dataSource={paginatedIndexes}
              columns={columns}
              rowKey="id"
              pagination={{
                current: currentPage,
                pageSize: pageSize,
                total: filteredIndexes.length,
                onChange: (page, size) => {
                  setCurrentPage(page);
                  if (size) setPageSize(size);
                },
                showSizeChanger: true,
                showTotal: (total) => `Total ${total} indexes`,
                pageSizeOptions: ['10', '20', '50', '100'],
              }}
              locale={{
                emptyText: searchText ? (
                  <Empty description={`No indexes found matching "${searchText}"`} />
                ) : (
                  <Empty description="No indexes found" />
                )
              }}
            />
          )}
        </Spin>
      </Card>

      {/* Create Index Modal */}
      <CreateIndexModal
        visible={createModalVisible}
        space={currentSpace}
        onCancel={() => setCreateModalVisible(false)}
        onSuccess={handleCreateSuccess}
      />

      {/* Detail Modal */}
      <Modal
        title="Index Details"
        open={detailModalVisible}
        onCancel={() => setDetailModalVisible(false)}
        footer={[
          <Button key="close" onClick={() => setDetailModalVisible(false)}>
            Close
          </Button>,
        ]}
        width={600}
      >
        {selectedIndex && (
          <div>
            <p>
              <Text strong>Name:</Text> {selectedIndex.name}
            </p>
            <p>
              <Text strong>Type:</Text>{' '}
              <Tag color={selectedIndex.entity_type === 'TAG' ? 'blue' : 'green'}>
                {selectedIndex.entity_type}
              </Tag>
            </p>
            <p>
              <Text strong>Entity:</Text> {selectedIndex.entity_name}
            </p>
            <p>
              <Text strong>Fields:</Text> {selectedIndex.fields.join(', ')}
            </p>
            <p>
              <Text strong>Status:</Text> {getStatusBadge(selectedIndex.status)}
            </p>
            <p>
              <Text strong>Created At:</Text>{' '}
              {new Date(selectedIndex.created_at * 1000).toLocaleString()}
            </p>
          </div>
        )}
      </Modal>
    </div>
  );
};

export default IndexList;

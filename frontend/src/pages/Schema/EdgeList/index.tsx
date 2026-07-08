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
  Input,
  Modal,
  Form,
  Select,
} from 'antd';
import {
  PlusOutlined,
  ReloadOutlined,
  EyeOutlined,
  DeleteOutlined,
  EditOutlined,
  ShareAltOutlined,
} from '@ant-design/icons';
import { useSchemaStore } from '@/stores/schema';
import type { EdgeType, PropertyDef } from '@/types/schema';
import EditModal from '../components/EditModal';
import TTLForm from '../components/TTLForm';
import styles from './index.module.less';

const { Title, Text } = Typography;

const DATA_TYPES = ['STRING', 'INT64', 'DOUBLE', 'BOOL', 'DATETIME', 'DATE', 'TIME', 'TIMESTAMP'];

const EdgeList: React.FC = () => {
  const {
    edgeTypes,
    isLoadingEdgeTypes,
    edgeTypesError,
    currentSpace,
    fetchEdgeTypes,
    createEdgeType,
    deleteEdgeType,
  } = useSchemaStore();

  const [searchText, setSearchText] = useState('');
  const [currentPage, setCurrentPage] = useState(1);
  const [pageSize, setPageSize] = useState(10);
  const [createModalVisible, setCreateModalVisible] = useState(false);
  const [detailModalVisible, setDetailModalVisible] = useState(false);
  const [editModalVisible, setEditModalVisible] = useState(false);
  const [selectedEdge, setSelectedEdge] = useState<EdgeType | null>(null);
  const [form] = Form.useForm();
  const [properties, setProperties] = useState<PropertyDef[]>([]);

  // Filtered and paginated edges
  const filteredEdges = useMemo(() => {
    if (!searchText.trim()) return edgeTypes;
    return edgeTypes.filter((edge) =>
      edge.name.toLowerCase().includes(searchText.toLowerCase())
    );
  }, [edgeTypes, searchText]);

  const paginatedEdges = useMemo(() => {
    const start = (currentPage - 1) * pageSize;
    return filteredEdges.slice(start, start + pageSize);
  }, [filteredEdges, currentPage, pageSize]);

  useEffect(() => {
    if (currentSpace) {
      fetchEdgeTypes(currentSpace);
    }
  }, [currentSpace, fetchEdgeTypes]);

  const handleRefresh = () => {
    if (currentSpace) {
      fetchEdgeTypes(currentSpace);
      setSearchText('');
      setCurrentPage(1);
      message.success('Edge type list refreshed');
    }
  };

  const handleSearchChange = (e: React.ChangeEvent<HTMLInputElement>) => {
    setSearchText(e.target.value);
    setCurrentPage(1);
  };

  const handleCreate = async () => {
    try {
      const values = await form.validateFields();
      if (currentSpace) {
        const params: { name: string; properties: PropertyDef[]; ttlCol?: string; ttlDuration?: number } = {
          name: values.name,
          properties: properties,
        };

        if (values.ttlCol && values.ttlDuration) {
          params.ttlCol = values.ttlCol;
          params.ttlDuration = values.ttlDuration;
        }

        await createEdgeType(currentSpace, params);
        message.success(`Edge type "${values.name}" created successfully`);
        setCreateModalVisible(false);
        form.resetFields();
        setProperties([]);
      }
    } catch (err: unknown) {
      const errorMessage = err instanceof Error ? err.message : 'Failed to create edge type';
      message.error(errorMessage);
    }
  };

  const handleDelete = async (edge: EdgeType) => {
    try {
      if (currentSpace) {
        await deleteEdgeType(currentSpace, edge.name);
        message.success(`Edge type "${edge.name}" deleted successfully`);
      }
    } catch (err: unknown) {
      const errorMessage = err instanceof Error ? err.message : 'Failed to delete edge type';
      message.error(errorMessage);
    }
  };

  const handleViewDetail = (edge: EdgeType) => {
    setSelectedEdge(edge);
    setDetailModalVisible(true);
  };

  const handleEdit = (edge: EdgeType) => {
    setSelectedEdge(edge);
    setEditModalVisible(true);
  };

  const handleEditSuccess = () => {
    setEditModalVisible(false);
    if (currentSpace) {
      fetchEdgeTypes(currentSpace);
    }
  };

  const handleAddProperty = () => {
    setProperties([...properties, { name: '', data_type: 'STRING', nullable: true }]);
  };

  const handleRemoveProperty = (index: number) => {
    setProperties(properties.filter((_, i) => i !== index));
  };

  const handlePropertyChange = (index: number, field: keyof PropertyDef, value: string | boolean) => {
    const newProperties = [...properties];
    newProperties[index] = { ...newProperties[index], [field]: value };
    setProperties(newProperties);
  };



  const columns = [
    {
      title: 'Name',
      dataIndex: 'name',
      key: 'name',
      render: (name: string) => (
        <AntSpace>
          <ShareAltOutlined />
          <Text strong>{name}</Text>
        </AntSpace>
      ),
      sorter: (a: EdgeType, b: EdgeType) => a.name.localeCompare(b.name),
    },
    {
      title: 'Properties',
      dataIndex: 'properties',
      key: 'properties',
      render: (properties: PropertyDef[]) => (
        <Text>{properties.length} properties</Text>
      ),
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
      render: (_: unknown, record: EdgeType) => (
        <AntSpace>
          <Tooltip title="View Details">
            <Button
              type="text"
              icon={<EyeOutlined />}
              onClick={() => handleViewDetail(record)}
            />
          </Tooltip>
          <Tooltip title="Edit">
            <Button
              type="text"
              icon={<EditOutlined />}
              onClick={() => handleEdit(record)}
            />
          </Tooltip>
          <Tooltip title="Delete">
            <Popconfirm
              title="Delete Edge Type"
              description={`Are you sure you want to delete edge type "${record.name}"?`}
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
            <Title level={4} style={{ margin: 0 }}>Edge Types</Title>
            <Text type="secondary">({filteredEdges.length})</Text>
          </AntSpace>
        }
        extra={
          <AntSpace>
            <Input.Search
              placeholder="Search edge types..."
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
              Create Edge
            </Button>
          </AntSpace>
        }
      >
        <Spin spinning={isLoadingEdgeTypes}>
          {edgeTypesError ? (
            <Empty description={edgeTypesError} />
          ) : (
            <Table
              dataSource={paginatedEdges}
              columns={columns}
              rowKey="id"
              pagination={{
                current: currentPage,
                pageSize: pageSize,
                total: filteredEdges.length,
                onChange: (page, size) => {
                  setCurrentPage(page);
                  if (size) setPageSize(size);
                },
                showSizeChanger: true,
                showTotal: (total) => `Total ${total} edge types`,
                pageSizeOptions: ['10', '20', '50', '100'],
              }}
              locale={{
                emptyText: searchText ? (
                  <Empty description={`No edge types found matching "${searchText}"`} />
                ) : (
                  <Empty description="No edge types found" />
                )
              }}
            />
          )}
        </Spin>
      </Card>

      {/* Create Edge Modal */}
      <Modal
        title="Create Edge Type"
        open={createModalVisible}
        onOk={handleCreate}
        onCancel={() => {
          setCreateModalVisible(false);
          form.resetFields();
          setProperties([]);
        }}
        width={700}
      >
        <Form form={form} layout="vertical">
          <Form.Item
            name="name"
            label="Edge Type Name"
            rules={[
              { required: true, message: 'Please enter edge type name' },
              { pattern: /^[a-zA-Z][a-zA-Z0-9_]*$/, message: 'Must start with letter, alphanumeric and underscores only' },
            ]}
          >
            <Input placeholder="Enter edge type name" />
          </Form.Item>

          <Form.Item label="Properties">
            <div className={styles.propertiesSection}>
              {properties.map((prop, index) => (
                <div key={index} className={styles.propertyRow}>
                  <Input
                    placeholder="Property name"
                    value={prop.name}
                    onChange={(e) => handlePropertyChange(index, 'name', e.target.value)}
                    style={{ width: 150 }}
                  />
                  <Select
                    value={prop.data_type}
                    onChange={(value) => handlePropertyChange(index, 'data_type', value)}
                    style={{ width: 120 }}
                    options={DATA_TYPES.map((type) => ({
                      label: type,
                      value: type,
                    }))}
                  />
                  <Input
                    placeholder="Default value (optional)"
                    value={prop.default_value || ''}
                    onChange={(e) => handlePropertyChange(index, 'default_value', e.target.value)}
                    style={{ width: 150 }}
                  />
                  <Button type="link" danger onClick={() => handleRemoveProperty(index)}>
                    Remove
                  </Button>
                </div>
              ))}
              <Button type="dashed" onClick={handleAddProperty} block>
                + Add Property
              </Button>
            </div>
          </Form.Item>

          <TTLForm form={form} properties={properties} />
        </Form>
      </Modal>

      {/* Detail Modal */}
      <Modal
        title="Edge Type Details"
        open={detailModalVisible}
        onCancel={() => setDetailModalVisible(false)}
        footer={[
          <Button key="close" onClick={() => setDetailModalVisible(false)}>
            Close
          </Button>,
        ]}
        width={600}
      >
        {selectedEdge && (
          <div>
            <p><Text strong>Name:</Text> {selectedEdge.name}</p>
            <p><Text strong>Created At:</Text> {new Date(selectedEdge.created_at * 1000).toLocaleString()}</p>
            <p><Text strong>Properties:</Text></p>
            <Table
              dataSource={selectedEdge.properties}
              columns={[
                { title: 'Name', dataIndex: 'name' },
                { title: 'Type', dataIndex: 'data_type' },
                { title: 'Default', dataIndex: 'default_value', render: (v: string) => v || '-' },
                { title: 'Nullable', dataIndex: 'nullable', render: (v: boolean) => v ? 'Yes' : 'No' },
              ]}
              pagination={false}
              size="small"
              rowKey="name"
            />
          </div>
        )}
      </Modal>

      {/* Edit Modal */}
      {selectedEdge && currentSpace && (
        <EditModal
          visible={editModalVisible}
          type="EDGE"
          name={selectedEdge.name}
          space={currentSpace}
          initialProperties={selectedEdge.properties}
          onCancel={() => setEditModalVisible(false)}
          onSuccess={handleEditSuccess}
        />
      )}
    </div>
  );
};

export default EdgeList;

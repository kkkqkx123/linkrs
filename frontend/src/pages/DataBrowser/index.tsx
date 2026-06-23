import React, { useEffect, useState } from 'react';
import {
  Card,
  Tabs,
  Select,
  Table,
  Pagination,
  Spin,
  Alert,
  Button,
  Space,
  Tooltip,
} from 'antd';
import {
  DatabaseOutlined,
  LinkOutlined,
  FilterOutlined,
  EyeOutlined,
  ReloadOutlined,
} from '@ant-design/icons';
import { useDataBrowserStore } from '@/stores/dataBrowser';
import { useSchemaStore } from '@/stores/schema';
import { dataBrowserService } from '@/services/dataBrowser';
import FilterPanel from './components/FilterPanel';
import StatisticsPanel from './components/StatisticsPanel';
import DetailModal from './components/DetailModal';
import type { VertexData, EdgeData } from '@/types/dataBrowser';
import type { TableColumnType } from 'antd';
import styles from './index.module.less';

const { TabPane } = Tabs;

const DataBrowser: React.FC = () => {
  const { currentSpace, tags, edgeTypes } = useSchemaStore();
  const {
    activeTab,
    selectedTag,
    selectedEdgeType,
    vertices,
    edges,
    vertexTotal,
    edgeTotal,
    vertexPage,
    edgePage,
    vertexPageSize,
    edgePageSize,
    vertexSort,
    edgeSort,
    filters,
    filterPanelVisible,
    loading,
    error,
    setActiveTab,
    setSelectedTag,
    setSelectedEdgeType,
    setVertices,
    setEdges,
    setVertexPage,
    setEdgePage,
    setVertexPageSize,
    setEdgePageSize,
    setVertexSort,
    setEdgeSort,
    toggleFilterPanel,
    setStatistics,
    showDetail,
    setLoading,
    setError,
  } = useDataBrowserStore();

  const [vertexProperties, setVertexProperties] = useState<string[]>([]);
  const [edgeProperties, setEdgeProperties] = useState<string[]>([]);

  // Load statistics on mount
  useEffect(() => {
    if (currentSpace) {
      loadStatistics();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentSpace]);

  // Load data when dependencies change
  useEffect(() => {
    if (currentSpace && selectedTag) {
      loadVertices();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentSpace, selectedTag, vertexPage, vertexPageSize, vertexSort, filters]);

  useEffect(() => {
    if (currentSpace && selectedEdgeType) {
      loadEdges();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [currentSpace, selectedEdgeType, edgePage, edgePageSize, edgeSort, filters]);

  // Update properties when tag/edge type changes
  useEffect(() => {
    if (selectedTag && vertices.length > 0) {
      const props = Object.keys(vertices[0].properties);
      setVertexProperties(props);
    } else {
      setVertexProperties([]);
    }
  }, [selectedTag, vertices]);

  useEffect(() => {
    if (selectedEdgeType && edges.length > 0) {
      const props = Object.keys(edges[0].properties);
      setEdgeProperties(props);
    } else {
      setEdgeProperties([]);
    }
  }, [selectedEdgeType, edges]);

  const loadStatistics = async () => {
    if (!currentSpace) return;
    try {
      const stats = await dataBrowserService.getStatistics(currentSpace);
      setStatistics(stats);
    } catch (err) {
      console.error('Failed to load statistics:', err);
    }
  };

  const loadVertices = async () => {
    if (!currentSpace || !selectedTag) return;

    setLoading(true);
    setError(null);

    try {
      const response = await dataBrowserService.getVertices(
        currentSpace,
        selectedTag,
        vertexPage,
        vertexPageSize,
        vertexSort || { field: 'id', order: 'asc' },
        filters
      );
      setVertices(response.data, response.total);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to load vertices');
    } finally {
      setLoading(false);
    }
  };

  const loadEdges = async () => {
    if (!currentSpace || !selectedEdgeType) return;

    setLoading(true);
    setError(null);

    try {
      const response = await dataBrowserService.getEdges(
        currentSpace,
        selectedEdgeType,
        edgePage,
        edgePageSize,
        edgeSort || { field: 'id', order: 'asc' },
        filters
      );
      setEdges(response.data, response.total);
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to load edges');
    } finally {
      setLoading(false);
    }
  };

  const handleTableChange = (
    _pagination: unknown,
    _filters: unknown,
    sorter: unknown
  ) => {
    const getField = (field: unknown): string => {
      if (Array.isArray(field)) {
        return String(field[0]);
      }
      return String(field ?? 'id');
    };

    const parseSorter = (s: unknown): { field: string; order: 'asc' | 'desc' } => {
      if (!s || typeof s !== 'object') {
        return { field: 'id', order: 'asc' };
      }
      const sorterObj = s as { field?: unknown; order?: 'ascend' | 'descend' | null };
      const field = getField(sorterObj.field);
      const order = sorterObj.order === 'ascend' ? 'asc' : 'desc';
      return { field, order };
    };

    const parsedSorter = parseSorter(sorter);

    if (activeTab === 'vertices') {
      setVertexSort(parsedSorter);
    } else {
      setEdgeSort(parsedSorter);
    }
  };

  const getVertexColumns = (): TableColumnType<VertexData>[] => {
    const baseColumns: TableColumnType<VertexData>[] = [
      {
        title: 'ID',
        dataIndex: 'id',
        key: 'id',
        width: 200,
        ellipsis: true,
        sorter: true,
      },
      {
        title: 'Tag',
        dataIndex: 'tag',
        key: 'tag',
        width: 120,
      },
    ];

    const propertyColumns = vertexProperties.map((prop) => ({
      title: prop,
      dataIndex: ['properties', prop],
      key: prop,
      ellipsis: true,
      sorter: true,
      render: (value: unknown) => String(value ?? '-'),
    }));

    const actionColumn: TableColumnType<VertexData> = {
      title: 'Action',
      key: 'action',
      width: 80,
      fixed: 'right',
      render: (_, record) => (
        <Tooltip title="View Detail">
          <Button
            icon={<EyeOutlined />}
            size="small"
            type="text"
            onClick={() => showDetail(record, 'vertex')}
          />
        </Tooltip>
      ),
    };

    return [...baseColumns, ...propertyColumns, actionColumn];
  };

  const getEdgeColumns = (): TableColumnType<EdgeData>[] => {
    const baseColumns: TableColumnType<EdgeData>[] = [
      {
        title: 'ID',
        dataIndex: 'id',
        key: 'id',
        width: 200,
        ellipsis: true,
        sorter: true,
      },
      {
        title: 'Type',
        dataIndex: 'type',
        key: 'type',
        width: 120,
      },
      {
        title: 'Source',
        dataIndex: 'src',
        key: 'src',
        width: 200,
        ellipsis: true,
      },
      {
        title: 'Target',
        dataIndex: 'dst',
        key: 'dst',
        width: 200,
        ellipsis: true,
      },
      {
        title: 'Rank',
        dataIndex: 'rank',
        key: 'rank',
        width: 80,
        sorter: true,
      },
    ];

    const propertyColumns = edgeProperties.map((prop) => ({
      title: prop,
      dataIndex: ['properties', prop],
      key: prop,
      ellipsis: true,
      sorter: true,
      render: (value: unknown) => String(value ?? '-'),
    }));

    const actionColumn: TableColumnType<EdgeData> = {
      title: 'Action',
      key: 'action',
      width: 80,
      fixed: 'right',
      render: (_, record) => (
        <Tooltip title="View Detail">
          <Button
            icon={<EyeOutlined />}
            size="small"
            type="text"
            onClick={() => showDetail(record, 'edge')}
          />
        </Tooltip>
      ),
    };

    return [...baseColumns, ...propertyColumns, actionColumn];
  };

  if (!currentSpace) {
    return (
      <div className={styles.container}>
        <Alert
          message="Please select a space first"
          description="You need to select a space to browse data"
          type="info"
          showIcon
        />
      </div>
    );
  }

  return (
    <div className={styles.container}>
      <Card className={styles.headerCard}>
        <div className={styles.header}>
          <h2>Data Browser</h2>
          <Space>
            <Button
              icon={<ReloadOutlined />}
              onClick={loadStatistics}
              loading={loading}
            >
              Refresh
            </Button>
            <Button
              icon={<FilterOutlined />}
              type={filterPanelVisible ? 'primary' : 'default'}
              onClick={toggleFilterPanel}
            >
              Filter
            </Button>
          </Space>
        </div>
      </Card>

      <div className={styles.content}>
        <div className={styles.main}>
          {error && (
            <Alert
              message={error}
              type="error"
              showIcon
              closable
              className={styles.error}
            />
          )}

          <Card className={styles.dataCard}>
            <Tabs activeKey={activeTab} onChange={(key) => setActiveTab(key as 'vertices' | 'edges')}>
              <TabPane
                tab={<span><DatabaseOutlined /> Vertices</span>}
                key="vertices"
              >
                <div className={styles.filterRow}>
                  <Select
                    placeholder="Select Tag"
                    value={selectedTag}
                    onChange={setSelectedTag}
                    style={{ width: 200 }}
                    allowClear
                    options={tags.map((tag) => ({
                      label: tag.name,
                      value: tag.name,
                    }))}
                  />
                </div>

                {filterPanelVisible && <FilterPanel properties={vertexProperties} />}

                <Spin spinning={loading}>
                  <Table
                    dataSource={vertices}
                    columns={getVertexColumns()}
                    rowKey="id"
                    pagination={false}
                    scroll={{ x: 'max-content' }}
                    size="small"
                    onChange={handleTableChange}
                  />
                  <div className={styles.pagination}>
                    <Pagination
                      current={vertexPage}
                      pageSize={vertexPageSize}
                      total={vertexTotal}
                      showSizeChanger
                      showTotal={(total) => `Total ${total} items`}
                      onChange={setVertexPage}
                      onShowSizeChange={(_, size) => setVertexPageSize(size)}
                    />
                  </div>
                </Spin>
              </TabPane>

              <TabPane
                tab={<span><LinkOutlined /> Edges</span>}
                key="edges"
              >
                <div className={styles.filterRow}>
                  <Select
                    placeholder="Select Edge Type"
                    value={selectedEdgeType}
                    onChange={setSelectedEdgeType}
                    style={{ width: 200 }}
                    allowClear
                    options={edgeTypes.map((type) => ({
                      label: type.name,
                      value: type.name,
                    }))}
                  />
                </div>

                {filterPanelVisible && <FilterPanel properties={edgeProperties} />}

                <Spin spinning={loading}>
                  <Table
                    dataSource={edges}
                    columns={getEdgeColumns()}
                    rowKey="id"
                    pagination={false}
                    scroll={{ x: 'max-content' }}
                    size="small"
                    onChange={handleTableChange}
                  />
                  <div className={styles.pagination}>
                    <Pagination
                      current={edgePage}
                      pageSize={edgePageSize}
                      total={edgeTotal}
                      showSizeChanger
                      showTotal={(total) => `Total ${total} items`}
                      onChange={setEdgePage}
                      onShowSizeChange={(_, size) => setEdgePageSize(size)}
                    />
                  </div>
                </Spin>
              </TabPane>
            </Tabs>
          </Card>
        </div>

        <div className={styles.sidebar}>
          <StatisticsPanel />
        </div>
      </div>

      <DetailModal />
    </div>
  );
};

export default DataBrowser;

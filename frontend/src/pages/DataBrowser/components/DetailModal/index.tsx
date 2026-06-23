import React from 'react';
import { Modal, Descriptions, Tag, Space, Button, Tooltip } from 'antd';
import { CopyOutlined } from '@ant-design/icons';
import { useDataBrowserStore } from '@/stores/dataBrowser';
import { copyToClipboard } from '@/utils/function';
import type { VertexData, EdgeData } from '@/types/dataBrowser';
import styles from './index.module.less';

const DetailModal: React.FC = () => {
  const {
    detailModalVisible,
    detailData,
    detailType,
    hideDetail,
  } = useDataBrowserStore();

  const handleCopyId = () => {
    if (detailData) {
      copyToClipboard(detailData.id);
    }
  };

  const renderVertexDetail = (vertex: VertexData) => (
    <Descriptions column={1} bordered>
      <Descriptions.Item label="ID">
        <Space>
          <span className={styles.idText}>{vertex.id}</span>
          <Tooltip title="Copy ID">
            <Button
              icon={<CopyOutlined />}
              size="small"
              type="text"
              onClick={handleCopyId}
            />
          </Tooltip>
        </Space>
      </Descriptions.Item>
      <Descriptions.Item label="Tag">
        <Tag color="blue">{vertex.tag}</Tag>
      </Descriptions.Item>
      {Object.entries(vertex.properties).map(([key, value]) => (
        <Descriptions.Item key={key} label={key}>
          {String(value)}
        </Descriptions.Item>
      ))}
    </Descriptions>
  );

  const renderEdgeDetail = (edge: EdgeData) => (
    <Descriptions column={1} bordered>
      <Descriptions.Item label="ID">
        <Space>
          <span className={styles.idText}>{edge.id}</span>
          <Tooltip title="Copy ID">
            <Button
              icon={<CopyOutlined />}
              size="small"
              type="text"
              onClick={handleCopyId}
            />
          </Tooltip>
        </Space>
      </Descriptions.Item>
      <Descriptions.Item label="Type">
        <Tag color="green">{edge.type}</Tag>
      </Descriptions.Item>
      <Descriptions.Item label="Source">
        <span className={styles.idText}>{edge.src}</span>
      </Descriptions.Item>
      <Descriptions.Item label="Target">
        <span className={styles.idText}>{edge.dst}</span>
      </Descriptions.Item>
      <Descriptions.Item label="Rank">{edge.rank}</Descriptions.Item>
      {Object.entries(edge.properties).map(([key, value]) => (
        <Descriptions.Item key={key} label={key}>
          {String(value)}
        </Descriptions.Item>
      ))}
    </Descriptions>
  );

  return (
    <Modal
      title={detailType === 'vertex' ? 'Vertex Detail' : 'Edge Detail'}
      open={detailModalVisible}
      onCancel={hideDetail}
      footer={null}
      width={600}
      className={styles.modal}
    >
      {detailData &&
        (detailType === 'vertex'
          ? renderVertexDetail(detailData as VertexData)
          : renderEdgeDetail(detailData as EdgeData))}
    </Modal>
  );
};

export default DetailModal;

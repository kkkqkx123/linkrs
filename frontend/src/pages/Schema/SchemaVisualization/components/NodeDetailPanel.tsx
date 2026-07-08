import React from 'react';
import { Card, Table, Tag, Button } from 'antd';
import { CloseOutlined } from '@ant-design/icons';
import type { NodeData } from '../utils/graphBuilder';
import styles from './index.module.less';

interface NodeDetailPanelProps {
  node: NodeData;
  onClose: () => void;
}

const NodeDetailPanel: React.FC<NodeDetailPanelProps> = ({ node, onClose }) => {
  const isTag = node.type === 'tag';

  const propertyColumns = [
    { title: 'Name', dataIndex: 'name', key: 'name' },
    {
      title: 'Type',
      dataIndex: 'type',
      key: 'type',
      render: (type: string) => <Tag color="blue">{type}</Tag>,
    },
  ];

  return (
    <Card
      className={styles.detailPanel}
      title={
        <div className={styles.header}>
          <span>
            {node.name}
            <Tag color={isTag ? 'green' : 'orange'} style={{ marginLeft: 8 }}>
              {isTag ? 'Tag' : 'Edge'}
            </Tag>
          </span>
          <Button type="text" size="small" icon={<CloseOutlined />} onClick={onClose} />
        </div>
      }
    >
      {node.properties && node.properties.length > 0 && (
        <div className={styles.section}>
          <h4>Properties</h4>
          <Table
            dataSource={node.properties}
            columns={propertyColumns}
            pagination={false}
            size="small"
            rowKey="name"
          />
        </div>
      )}

      {node.comment && (
        <div className={styles.section}>
          <h4>Comment</h4>
          <p className={styles.comment}>{node.comment}</p>
        </div>
      )}

      {node.ttl && (
        <div className={styles.section}>
          <h4>TTL Configuration</h4>
          <p>Duration: {node.ttl.duration}s</p>
          <p>Column: {node.ttl.col}</p>
        </div>
      )}
    </Card>
  );
};

export default NodeDetailPanel;

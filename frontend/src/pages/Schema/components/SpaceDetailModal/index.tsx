import React, { useEffect } from 'react';
import { Modal, Descriptions, Button, Statistic, Row, Col, Spin } from 'antd';
import { ReloadOutlined } from '@ant-design/icons';
import { useSchemaStore } from '@/stores/schema';
import type { Space } from '@/types/schema';
import styles from './index.module.less';

interface SpaceDetailModalProps {
  visible: boolean;
  space: Space | null;
  onClose: () => void;
}

const SpaceDetailModal: React.FC<SpaceDetailModalProps> = ({
  visible,
  space,
  onClose,
}) => {
  const {
    spaceDetails,
    spaceStatistics,
    fetchSpaceDetail,
    fetchSpaceStatistics,
  } = useSchemaStore();

  useEffect(() => {
    if (visible && space) {
      fetchSpaceDetail(space.name);
      fetchSpaceStatistics(space.name);
    }
  }, [visible, space, fetchSpaceDetail, fetchSpaceStatistics]);

  const handleRefreshStats = () => {
    if (space) {
      fetchSpaceStatistics(space.name);
    }
  };

  if (!space) return null;

  const detail = spaceDetails[space.name];
  const stats = spaceStatistics[space.name];

  const formatDate = (timestamp: number | undefined) => {
    if (!timestamp) return 'N/A';
    return new Date(timestamp * 1000).toLocaleString();
  };

  return (
    <Modal
      title={`Space Details: ${space.name}`}
      open={visible}
      onCancel={onClose}
      footer={[
        <Button key="close" onClick={onClose}>
          Close
        </Button>,
      ]}
      width={600}
    >
      <Spin spinning={!detail}>
        <div className={styles.container}>
          {/* Basic Info Section */}
          <div className={styles.section}>
            <h4 className={styles.sectionTitle}>Basic Information</h4>
            <Descriptions bordered column={1} size="small">
              <Descriptions.Item label="Name">{space.name}</Descriptions.Item>
              <Descriptions.Item label="Created At">
                {formatDate(detail?.created_at)}
              </Descriptions.Item>
              {detail?.comment && (
                <Descriptions.Item label="Comment">{detail.comment}</Descriptions.Item>
              )}
            </Descriptions>
          </div>

          {/* Configuration Section */}
          <div className={styles.section}>
            <h4 className={styles.sectionTitle}>Configuration</h4>
            <Descriptions bordered column={1} size="small">
              <Descriptions.Item label="Vid Type">
                {space.vid_type}
              </Descriptions.Item>
              <Descriptions.Item label="Partition Number">
                {detail?.partition_num || 'N/A'}
              </Descriptions.Item>
              <Descriptions.Item label="Replica Factor">
                {detail?.replica_factor || 'N/A'}
              </Descriptions.Item>
            </Descriptions>
          </div>

          {/* Statistics Section */}
          <div className={styles.section}>
            <div className={styles.statsHeader}>
              <h4 className={styles.sectionTitle}>Statistics</h4>
              <Button
                icon={<ReloadOutlined />}
                size="small"
                onClick={handleRefreshStats}
              >
                Refresh
              </Button>
            </div>
            <Row gutter={16}>
              <Col span={12}>
                <Statistic
                  title="Vertices"
                  value={stats?.vertex_count || 0}
                  loading={!stats}
                />
              </Col>
              <Col span={12}>
                <Statistic
                  title="Edges"
                  value={stats?.edge_count || 0}
                  loading={!stats}
                />
              </Col>
            </Row>
          </div>
        </div>
      </Spin>
    </Modal>
  );
};

export default SpaceDetailModal;

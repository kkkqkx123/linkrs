import React from 'react';
import { Card, Row, Col, Statistic, Progress, Empty } from 'antd';
import { DatabaseOutlined, LinkOutlined, TagOutlined, BranchesOutlined } from '@ant-design/icons';
import { useDataBrowserStore } from '@/stores/dataBrowser';
import styles from './index.module.less';

const StatisticsPanel: React.FC = () => {
  const { statistics } = useDataBrowserStore();

  if (!statistics) {
    return (
      <Card title="Statistics" size="small" className={styles.panel}>
        <Empty description="No statistics available" image={Empty.PRESENTED_IMAGE_SIMPLE} />
      </Card>
    );
  }

  const maxTagCount = Math.max(...statistics.tagDistribution.map((t) => t.count), 1);
  const maxEdgeTypeCount = Math.max(
    ...statistics.edgeTypeDistribution.map((t) => t.count),
    1
  );

  return (
    <Card title="Statistics" size="small" className={styles.panel}>
      <Row gutter={16} className={styles.overview}>
        <Col span={12}>
          <Statistic
            title="Vertices"
            value={statistics.totalVertices}
            prefix={<DatabaseOutlined />}
          />
        </Col>
        <Col span={12}>
          <Statistic
            title="Edges"
            value={statistics.totalEdges}
            prefix={<LinkOutlined />}
          />
        </Col>
        <Col span={12}>
          <Statistic
            title="Tags"
            value={statistics.tagCount}
            prefix={<TagOutlined />}
          />
        </Col>
        <Col span={12}>
          <Statistic
            title="Edge Types"
            value={statistics.edgeTypeCount}
            prefix={<BranchesOutlined />}
          />
        </Col>
      </Row>

      {statistics.tagDistribution.length > 0 && (
        <div className={styles.distribution}>
          <div className={styles.sectionTitle}>Tag Distribution</div>
          {statistics.tagDistribution.map((tag) => (
            <div key={tag.tag} className={styles.progressItem}>
              <div className={styles.progressLabel}>
                <span>{tag.tag}</span>
                <span>{tag.count}</span>
              </div>
              <Progress
                percent={Math.round((tag.count / maxTagCount) * 100)}
                showInfo={false}
                size="small"
              />
            </div>
          ))}
        </div>
      )}

      {statistics.edgeTypeDistribution.length > 0 && (
        <div className={styles.distribution}>
          <div className={styles.sectionTitle}>Edge Type Distribution</div>
          {statistics.edgeTypeDistribution.map((type) => (
            <div key={type.type} className={styles.progressItem}>
              <div className={styles.progressLabel}>
                <span>{type.type}</span>
                <span>{type.count}</span>
              </div>
              <Progress
                percent={Math.round((type.count / maxEdgeTypeCount) * 100)}
                showInfo={false}
                size="small"
              />
            </div>
          ))}
        </div>
      )}
    </Card>
  );
};

export default StatisticsPanel;

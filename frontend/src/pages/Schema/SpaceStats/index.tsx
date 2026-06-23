import React, { useEffect, useState, useCallback, useRef } from 'react';
import { Card, Table, Button, Spin, message, Badge, Statistic, Row, Col, Empty } from 'antd';
import { ReloadOutlined, ClockCircleOutlined } from '@ant-design/icons';
import { useSchemaStore } from '@/stores/schema';
import { useTranslation } from 'react-i18next';
import styles from './index.module.less';

export type JobStatus = 'QUEUE' | 'RUNNING' | 'FINISHED' | 'FAILED';

interface StatsItem {
  type: string;
  name: string;
  count: number;
}

interface StatsData {
  tags: StatsItem[];
  edges: StatsItem[];
  totalVertices: number;
  totalEdges: number;
}

interface QueryRow {
  Type?: string;
  Name?: string;
  Count?: number;
  Status?: string;
  'New Job Id'?: number;
  [key: string]: unknown;
}

const SpaceStats: React.FC = () => {
  const { t } = useTranslation();
  const { currentSpace } = useSchemaStore();
  const [loading, setLoading] = useState(false);
  const [statsData, setStatsData] = useState<StatsData | null>(null);
  const [lastUpdated, setLastUpdated] = useState<string>('');
  const [jobStatus, setJobStatus] = useState<JobStatus | null>(null);
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const fetchStats = useCallback(async () => {
    if (!currentSpace) return;

    try {
      const { queryService } = await import('@/services/query');
      const result = await queryService.execute({
        query: 'SHOW STATS',
      });

      if (result.data && result.data.rows && result.data.rows.length > 0) {
        const data = processStatsData(result.data.rows[0] as unknown as QueryRow[]);
        setStatsData(data);
        setLastUpdated(new Date().toLocaleString());
      }
    } catch (err) {
      console.error('Failed to fetch stats:', err);
    }
  }, [currentSpace]);

  const checkJobStatus = useCallback(async (jobId: number) => {
    if (!currentSpace) return;

    try {
      const { queryService } = await import('@/services/query');
      const result = await queryService.execute({
        query: `SHOW JOB ${jobId}`,
      });

      if (result.data && result.data.rows && result.data.rows.length > 0) {
        const row = result.data.rows[0] as unknown as QueryRow;
        const status = (row.Status || row.status) as JobStatus;
        setJobStatus(status);

        if (status === 'FINISHED') {
          await fetchStats();
          if (timerRef.current) {
            clearTimeout(timerRef.current);
            timerRef.current = null;
          }
        } else if (status === 'RUNNING' || status === 'QUEUE') {
          timerRef.current = setTimeout(() => checkJobStatus(jobId), 2000);
        }
      }
    } catch (err) {
      console.error('Failed to check job status:', err);
    }
  }, [currentSpace, fetchStats]);

  const handleSubmitStats = async () => {
    if (!currentSpace) {
      message.warning(t('schema.selectSpaceFirst'));
      return;
    }

    setLoading(true);
    try {
      const { queryService } = await import('@/services/query');
      const result = await queryService.execute({
        query: 'SUBMIT JOB STATS',
      });

      if (result.data && result.data.rows && result.data.rows.length > 0) {
        const row = result.data.rows[0] as unknown as QueryRow;
        const jobId = row['New Job Id'] || row.job_id;
        if (jobId) {
          message.success(t('schema.statsJobSubmitted'));
          setJobStatus('QUEUE');
          checkJobStatus(Number(jobId));
        }
      }
    } catch {
      message.error(t('schema.statsJobFailed'));
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    fetchStats();
    return () => {
      if (timerRef.current) {
        clearTimeout(timerRef.current);
      }
    };
  }, [fetchStats]);

  const processStatsData = (rawData: QueryRow[]): StatsData => {
    const tags: StatsItem[] = [];
    const edges: StatsItem[] = [];
    let totalVertices = 0;
    let totalEdges = 0;

    rawData.forEach((item) => {
      const type = item.Type || item.type;
      const name = item.Name || item.name;
      const count = item.Count || item.count;

      if (type === 'Tag') {
        tags.push({ type: 'Tag', name: String(name), count: Number(count) });
        totalVertices += Number(count);
      } else if (type === 'Edge') {
        edges.push({ type: 'Edge', name: String(name), count: Number(count) });
        totalEdges += Number(count);
      }
    });

    return { tags, edges, totalVertices, totalEdges };
  };

  const getStatusBadge = () => {
    switch (jobStatus) {
      case 'FINISHED':
        return <Badge status="success" text={t('common.finished')} />;
      case 'RUNNING':
        return <Badge status="processing" text={t('common.running')} />;
      case 'QUEUE':
        return <Badge status="warning" text={t('common.queued')} />;
      case 'FAILED':
        return <Badge status="error" text={t('common.failed')} />;
      default:
        return <Badge status="default" text={t('common.unknown')} />;
    }
  };

  const tagColumns = [
    { title: t('common.type'), dataIndex: 'type', key: 'type' },
    { title: t('common.name'), dataIndex: 'name', key: 'name' },
    {
      title: t('common.count'),
      dataIndex: 'count',
      key: 'count',
      align: 'right' as const,
      render: (count: number) => count.toLocaleString(),
    },
  ];

  const edgeColumns = [
    { title: t('common.type'), dataIndex: 'type', key: 'type' },
    { title: t('common.name'), dataIndex: 'name', key: 'name' },
    {
      title: t('common.count'),
      dataIndex: 'count',
      key: 'count',
      align: 'right' as const,
      render: (count: number) => count.toLocaleString(),
    },
  ];

  if (!currentSpace) {
    return (
      <Card>
        <Empty description={t('schema.selectSpaceFirst')} />
      </Card>
    );
  }

  return (
    <div className={styles.container}>
      <Card
        title={`${t('schema.spaceStats')}: ${currentSpace}`}
        extra={
          <Button
            type="primary"
            icon={<ReloadOutlined />}
            onClick={handleSubmitStats}
            loading={loading || jobStatus === 'RUNNING' || jobStatus === 'QUEUE'}
          >
            {t('schema.refreshStats')}
          </Button>
        }
      >
        <Spin spinning={loading}>
          <div className={styles.header}>
            <div className={styles.metaInfo}>
              <span>
                <ClockCircleOutlined /> {t('schema.lastUpdated')}: {lastUpdated || t('common.never')}
              </span>
              <span className={styles.status}>
                {t('common.status')}: {getStatusBadge()}
              </span>
            </div>
          </div>

          {statsData && (
            <>
              <Row gutter={16} className={styles.summary}>
                <Col span={12}>
                  <Statistic
                    title={t('dataBrowser.totalNodes')}
                    value={statsData.totalVertices}
                    formatter={(value) => (value as number)?.toLocaleString()}
                  />
                </Col>
                <Col span={12}>
                  <Statistic
                    title={t('dataBrowser.totalEdges')}
                    value={statsData.totalEdges}
                    formatter={(value) => (value as number)?.toLocaleString()}
                  />
                </Col>
              </Row>

              <div className={styles.tables}>
                <h3>{t('schema.tagStats')}</h3>
                <Table
                  dataSource={statsData.tags}
                  columns={tagColumns}
                  pagination={false}
                  size="small"
                  rowKey="name"
                />

                <h3>{t('schema.edgeStats')}</h3>
                <Table
                  dataSource={statsData.edges}
                  columns={edgeColumns}
                  pagination={false}
                  size="small"
                  rowKey="name"
                />
              </div>
            </>
          )}
        </Spin>
      </Card>
    </div>
  );
};

export default SpaceStats;

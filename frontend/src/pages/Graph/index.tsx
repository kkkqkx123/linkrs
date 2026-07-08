import React, { useState } from 'react';
import { Card, Input, Button, Select, Space, Alert } from 'antd';
import { PlayCircleOutlined } from '@ant-design/icons';
import GraphCanvas from '@/components/business/GraphCanvas';
import GraphToolbar from '@/components/business/GraphToolbar';
import StylePanel from '@/components/business/StylePanel';
import DetailPanel from '@/components/business/DetailPanel';
import { useGraphStore } from '@/stores/graph';
import { useSchemaStore } from '@/stores/schema';
import { queryService } from '@/services/query';
import { parseQueryResultToGraph } from '@/utils/cytoscapeConfig';
import styles from './index.module.less';

const { TextArea } = Input;

const QUERY_TEMPLATES = [
  { label: 'Show all nodes (limit 50)', value: 'MATCH (n) RETURN n LIMIT 50' },
  { label: 'Show all relationships (limit 50)', value: 'MATCH ()-[r]->() RETURN r LIMIT 50' },
  { label: 'Show graph (limit 100)', value: 'MATCH (n)-[r]->(m) RETURN n, r, m LIMIT 100' },
  { label: 'Find node by property', value: 'MATCH (n {name: "example"}) RETURN n' },
];

const GraphPage: React.FC = () => {
  const [query, setQuery] = useState('MATCH (n)-[r]->(m) RETURN n, r, m LIMIT 50');
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const { currentSpace } = useSchemaStore();
  const { graphData, setGraphData, clearGraphData } = useGraphStore();

  const handleExecute = async () => {
    if (!query.trim()) return;
    if (!currentSpace) {
      setError('Please select a space first');
      return;
    }

    setLoading(true);
    setError(null);
    clearGraphData();

    try {
      const response = await queryService.execute({ query, space: currentSpace });

      if (response.success && response.data) {
        const parsed = parseQueryResultToGraph(response.data.data);
        if (parsed.nodes.length > 0) {
          setGraphData(parsed);
        } else {
          setError('No graph data found in query result');
        }
      } else {
        setError(response.error?.message || 'Query execution failed');
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : 'Failed to execute query');
    } finally {
      setLoading(false);
    }
  };

  return (
    <div className={styles.graphPage}>
      {/* Query Panel */}
      <Card className={styles.queryCard}>
        <div className={styles.queryHeader}>
          <h3>Graph Visualization</h3>
          <Space>
            <Select
              placeholder="Select template"
              onChange={(value) => setQuery(value)}
              style={{ width: 250 }}
              allowClear
              options={QUERY_TEMPLATES.map((t) => ({
                label: t.label,
                value: t.value,
              }))}
            />
          </Space>
        </div>

        <TextArea
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          rows={4}
          placeholder="Enter Cypher query..."
          className={styles.queryInput}
        />

        <div className={styles.queryActions}>
          <Button
            type="primary"
            icon={<PlayCircleOutlined />}
            onClick={handleExecute}
            loading={loading}
            disabled={!currentSpace}
          >
            Execute
          </Button>
          {!currentSpace && (
            <span className={styles.hint}>Please select a space first</span>
          )}
        </div>

        {error && (
          <Alert
            message={error}
            type="error"
            showIcon
            closable
            onClose={() => setError(null)}
            className={styles.error}
          />
        )}
      </Card>

      {/* Graph Visualization */}
      {graphData && graphData.nodes.length > 0 ? (
        <div className={styles.visualization}>
          <div className={styles.graphArea}>
            <GraphCanvas height="600px" />
            <GraphToolbar />
          </div>
          <div className={styles.sidePanels}>
            <StylePanel />
            <DetailPanel />
          </div>
        </div>
      ) : (
        <Card className={styles.emptyCard}>
          <div className={styles.empty}>
            <p>Execute a query to visualize the graph</p>
            <p className={styles.hint}>
              Try: MATCH (n)-[r]-&gt;(m) RETURN n, r, m LIMIT 50
            </p>
          </div>
        </Card>
      )}
    </div>
  );
};

export default GraphPage;

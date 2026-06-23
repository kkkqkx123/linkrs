import React from 'react';
import { Alert } from 'antd';
import GraphCanvas from '@/components/business/GraphCanvas';
import GraphToolbar from '@/components/business/GraphToolbar';
import StylePanel from '@/components/business/StylePanel';
import DetailPanel from '@/components/business/DetailPanel';
import { useGraphStore } from '@/stores/graph';
import { parseQueryResultToGraph } from '@/utils/cytoscapeConfig';
import styles from './index.module.less';

interface GraphViewProps {
  data: unknown[];
}

const GraphView: React.FC<GraphViewProps> = ({ data }) => {
  const { graphData } = useGraphStore();

  // Parse data if not already parsed
  React.useEffect(() => {
    if (data && data.length > 0) {
      const parsed = parseQueryResultToGraph(data);
      if (parsed.nodes.length > 0) {
        useGraphStore.getState().setGraphData(parsed);
      }
    }
  }, [data]);

  const currentData = graphData || parseQueryResultToGraph(data);

  // Warning for large graphs
  const showWarning = currentData.nodes.length > 500;

  if (currentData.nodes.length === 0) {
    return (
      <div className={styles.empty}>
        <p>No graph data available in the query result.</p>
        <p className={styles.hint}>
          Try a query like: MATCH (n)-[r]-&gt;(m) RETURN n, r, m LIMIT 50
        </p>
      </div>
    );
  }

  return (
    <div className={styles.container}>
      {showWarning && (
        <Alert
          message={`Large graph warning: ${currentData.nodes.length} nodes and ${currentData.edges.length} edges. Performance may be affected.`}
          type="warning"
          showIcon
          closable
          className={styles.warning}
        />
      )}
      <div className={styles.content}>
        <div className={styles.main}>
          <GraphCanvas height="450px" />
          <GraphToolbar />
        </div>
        <div className={styles.sidePanels}>
          <StylePanel />
          <DetailPanel />
        </div>
      </div>
    </div>
  );
};

export default GraphView;

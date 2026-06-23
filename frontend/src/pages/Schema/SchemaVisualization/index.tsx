import React, { useEffect, useState, useCallback, useRef } from 'react';
import { Button, Spin, message, Empty } from 'antd';
import { ReloadOutlined, ExportOutlined } from '@ant-design/icons';
import { useSchemaStore } from '@/stores/schema';
import GraphCanvas from './components/GraphCanvas';
import NodeDetailPanel from './components/NodeDetailPanel';
import ZoomControls from './components/ZoomControls';
import Legend from './components/Legend';
import { buildSchemaGraph, type NodeData, type GraphData } from './utils/graphBuilder';
import styles from './index.module.less';

const SchemaVisualization: React.FC = () => {
  const { currentSpace, tags, edgeTypes, fetchTags, fetchEdgeTypes } = useSchemaStore();

  const [loading, setLoading] = useState(false);
  const [graphData, setGraphData] = useState<GraphData | null>(null);
  const [selectedNode, setSelectedNode] = useState<NodeData | null>(null);
  const [zoom, setZoom] = useState(100);
  const canvasRef = useRef<HTMLDivElement>(null);
  const cyRef = useRef<cytoscape.Core | null>(null);

  // Load schema data and build graph
  const loadSchemaGraph = useCallback(async () => {
    if (!currentSpace) {
      message.warning('Please select a space first');
      return;
    }

    setLoading(true);
    try {
      // Fetch Tags and Edges
      await Promise.all([fetchTags(currentSpace), fetchEdgeTypes(currentSpace)]);

      // Build graph data
      const data = buildSchemaGraph(tags, edgeTypes);
      setGraphData(data);
    } catch {
      message.error('Failed to load schema visualization');
    } finally {
      setLoading(false);
    }
  }, [currentSpace, fetchTags, fetchEdgeTypes, tags, edgeTypes]);

  useEffect(() => {
    loadSchemaGraph();
  }, [loadSchemaGraph]);

  // Handle node click
  const handleNodeClick = useCallback((node: NodeData | null) => {
    setSelectedNode(node);
  }, []);

  // Export image
  const handleExport = useCallback(() => {
    const canvas = canvasRef.current?.querySelector('canvas');
    if (canvas) {
      const link = document.createElement('a');
      link.download = `${currentSpace}_schema.png`;
      link.href = (canvas as HTMLCanvasElement).toDataURL('image/png');
      link.click();
    }
  }, [currentSpace]);

  // Zoom controls
  const handleZoomIn = useCallback(() => {
    if (cyRef.current) {
      const newZoom = cyRef.current.zoom() * 1.2;
      cyRef.current.zoom(newZoom);
      setZoom(Math.round(newZoom * 100));
    }
  }, []);

  const handleZoomOut = useCallback(() => {
    if (cyRef.current) {
      const newZoom = cyRef.current.zoom() / 1.2;
      cyRef.current.zoom(newZoom);
      setZoom(Math.round(newZoom * 100));
    }
  }, []);

  const handleFit = useCallback(() => {
    if (cyRef.current) {
      cyRef.current.fit();
      setZoom(Math.round(cyRef.current.zoom() * 100));
    }
  }, []);

  if (!currentSpace) {
    return <Empty description="Please select a space to view schema visualization" />;
  }

  return (
    <div className={styles.container}>
      <div className={styles.header}>
        <h2>Schema Visualization: {currentSpace}</h2>
        <div className={styles.actions}>
          <Button icon={<ReloadOutlined />} onClick={loadSchemaGraph} loading={loading}>
            Refresh
          </Button>
          <Button type="primary" icon={<ExportOutlined />} onClick={handleExport}>
            Export
          </Button>
        </div>
      </div>

      <div className={styles.content}>
        <Spin spinning={loading} tip="Loading schema visualization...">
          <div className={styles.canvasWrapper} ref={canvasRef}>
            {graphData && <GraphCanvas data={graphData} onNodeClick={handleNodeClick} />}
          </div>
        </Spin>

        <div className={styles.sidebar}>
          <Legend />
          {selectedNode && <NodeDetailPanel node={selectedNode} onClose={() => setSelectedNode(null)} />}
        </div>
      </div>

      <div className={styles.zoomControlsWrapper}>
        <ZoomControls onZoomIn={handleZoomIn} onZoomOut={handleZoomOut} onFit={handleFit} zoom={zoom} />
      </div>
    </div>
  );
};

export default SchemaVisualization;

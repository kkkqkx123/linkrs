import React from 'react';
import { Space, Select } from 'antd';
import {
  ZoomInOutlined,
  ZoomOutOutlined,
  ExpandOutlined,
  ReloadOutlined,
  ClearOutlined,
} from '@ant-design/icons';
import IconButton from '@/components/common/IconButton';
import { useGraphStore } from '@/stores/graph';
import { getLayoutOptions } from '@/utils/graphLayout';
import styles from './index.module.less';

interface GraphToolbarProps {
  cy?: cytoscape.Core;
}

const GraphToolbar: React.FC<GraphToolbarProps> = ({ cy }) => {
  const {
    layout,
    zoom,
    selectedNodes,
    selectedEdges,
    setLayout,
    fitToScreen,
    resetLayout,
    clearSelection,
  } = useGraphStore();

  const handleZoomIn = () => {
    if (cy) {
      cy.zoom(cy.zoom() * 1.2);
    }
  };

  const handleZoomOut = () => {
    if (cy) {
      cy.zoom(cy.zoom() * 0.8);
    }
  };

  const handleFit = () => {
    fitToScreen(cy);
  };

  const handleReset = () => {
    resetLayout(cy);
  };

  const selectionCount = selectedNodes.length + selectedEdges.length;

  return (
    <div className={styles.toolbar}>
      <Space>
        <IconButton
          title="Zoom In"
          icon={<ZoomInOutlined />}
          onClick={handleZoomIn}
        />
        <IconButton
          title="Zoom Out"
          icon={<ZoomOutOutlined />}
          onClick={handleZoomOut}
        />
        <span className={styles.zoomLevel}>{Math.round(zoom * 100)}%</span>
        <div className={styles.divider} />
        <IconButton
          title="Fit to Screen"
          icon={<ExpandOutlined />}
          onClick={handleFit}
        >
          Fit
        </IconButton>
        <IconButton
          title="Reset Layout"
          icon={<ReloadOutlined />}
          onClick={handleReset}
        >
          Reset
        </IconButton>
        <div className={styles.divider} />
        <Select
          value={layout}
          onChange={setLayout}
          options={getLayoutOptions()}
          size="small"
          style={{ width: 140 }}
        />
        {selectionCount > 0 && (
          <>
            <div className={styles.divider} />
            <IconButton
              title="Clear Selection"
              icon={<ClearOutlined />}
              onClick={clearSelection}
            >
              Clear ({selectionCount})
            </IconButton>
          </>
        )}
      </Space>
    </div>
  );
};

export default GraphToolbar;

import React from 'react';
import { Button, Space } from 'antd';
import { ZoomInOutlined, ZoomOutOutlined, ExpandOutlined } from '@ant-design/icons';
import styles from './index.module.less';

interface ZoomControlsProps {
  onZoomIn?: () => void;
  onZoomOut?: () => void;
  onFit?: () => void;
  zoom?: number;
}

const ZoomControls: React.FC<ZoomControlsProps> = ({ onZoomIn, onZoomOut, onFit, zoom = 100 }) => {
  return (
    <div className={styles.zoomControls}>
      <Space>
        <Button icon={<ZoomOutOutlined />} onClick={onZoomOut} size="small" />
        <span className={styles.zoomLevel}>{Math.round(zoom)}%</span>
        <Button icon={<ZoomInOutlined />} onClick={onZoomIn} size="small" />
        <Button icon={<ExpandOutlined />} onClick={onFit} size="small" title="Fit to screen" />
      </Space>
    </div>
  );
};

export default ZoomControls;

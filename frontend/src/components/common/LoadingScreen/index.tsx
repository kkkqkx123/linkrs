import React from 'react';
import { Spin } from 'antd';
import styles from './index.module.less';

interface LoadingScreenProps {
  size?: 'small' | 'default' | 'large';
  tip?: string;
  fullScreen?: boolean;
}

const LoadingScreen: React.FC<LoadingScreenProps> = ({
  size = 'large',
  tip,
  fullScreen = true,
}) => {
  return (
    <div
      className={`${styles.loadingScreen} ${fullScreen ? styles.fullScreen : ''}`}
    >
      <Spin size={size} tip={tip} />
    </div>
  );
};

export default LoadingScreen;

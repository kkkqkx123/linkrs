import React from 'react';
import { Card } from 'antd';
import styles from './index.module.less';

const Legend: React.FC = () => {
  return (
    <Card className={styles.legend} title="Legend" size="small">
      <div className={styles.legendItem}>
        <span className={`${styles.legendIcon} ${styles.tagIcon}`} />
        <span>Tag</span>
      </div>
      <div className={styles.legendItem}>
        <span className={`${styles.legendIcon} ${styles.edgeIcon}`} />
        <span>Edge Type</span>
      </div>
      <div className={styles.legendItem}>
        <span className={`${styles.legendIcon} ${styles.selectedIcon}`} />
        <span>Selected</span>
      </div>
    </Card>
  );
};

export default Legend;

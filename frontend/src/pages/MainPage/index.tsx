import React from 'react';
import { Card, Typography } from 'antd';
import styles from './index.module.less';

const { Title, Paragraph } = Typography;

const MainPage: React.FC = () => {
  return (
    <div className={styles.container}>
      <Card className={styles.welcomeCard}>
        <Title level={2}>Welcome to GraphDB Studio</Title>
        <Paragraph>
          Select a module from the sidebar to get started.
        </Paragraph>
        <Paragraph>
          <strong>Available Modules:</strong>
        </Paragraph>
        <ul>
          <li><strong>Console:</strong> Execute Cypher queries and view results</li>
          <li><strong>Schema:</strong> Manage tags, edges, and indexes</li>
          <li><strong>Graph:</strong> Visualize graph data</li>
          <li><strong>Data Browser:</strong> Browse vertices and edges</li>
        </ul>
      </Card>
    </div>
  );
};

export default MainPage;

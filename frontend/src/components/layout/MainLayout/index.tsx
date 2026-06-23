import React, { useEffect, useRef } from 'react';
import { Layout } from 'antd';
import Header from '../Header';
import Sidebar from '../Sidebar';
import LoadingScreen from '@/components/common/LoadingScreen';
import { useHealthCheck } from '@/hooks/useHealthCheck';
import { useConnectionStore } from '@/stores/connection';
import styles from './index.module.less';

const { Content } = Layout;

const MainLayout: React.FC<{ children: React.ReactNode }> = ({ children }) => {
  const { checkHealth, isVerified, connectionInfo, rememberMe, login } = useConnectionStore();
  const [isInitialCheck, setIsInitialCheck] = React.useState(true);
  const hasInitialized = useRef(false);

  useEffect(() => {
    if (hasInitialized.current) {
      return;
    }
    hasInitialized.current = true;

    const performInitialCheck = async () => {
      if (isVerified) {
        setIsInitialCheck(false);
        return;
      }

      if (rememberMe && connectionInfo.username && connectionInfo.password) {
        try {
          await login(connectionInfo.username, connectionInfo.password, true);
        } catch (error) {
          console.error('Auto login failed:', error);
        }
      } else {
        await checkHealth();
      }

      setIsInitialCheck(false);
    };

    performInitialCheck();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useHealthCheck(true);

  if (isInitialCheck) {
    return <LoadingScreen />;
  }

  return (
    <Layout className={styles.layout}>
      <Header />
      <Layout className={styles.mainLayout}>
        <Sidebar />
        <Content className={styles.content}>{children}</Content>
      </Layout>
    </Layout>
  );
};

export default MainLayout;

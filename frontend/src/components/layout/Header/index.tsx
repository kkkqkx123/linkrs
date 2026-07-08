import React from 'react';
import { Layout, Button, Space, Badge, Dropdown, Divider } from 'antd';
import { DatabaseOutlined, LogoutOutlined, UserOutlined, DisconnectOutlined } from '@ant-design/icons';
import { useNavigate } from 'react-router-dom';
import { useTranslation } from 'react-i18next';
import { useConnectionStore } from '@/stores/connection';
import SpaceSelector from '@/components/business/SpaceSelector';
import LanguageSwitcher from '@/components/common/LanguageSwitcher';
import styles from './index.module.less';

const { Header: AntHeader } = Layout;

const Header: React.FC = () => {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const { isVerified, connectionInfo, logout, isLoading } = useConnectionStore();

  const handleLogout = async () => {
    try {
      await logout();
      navigate('/login');
    } catch (error) {
      console.error('Logout error:', error);
    }
  };

  const menuItems = [
    {
      key: 'logout',
      label: t('common.logout'),
      icon: <LogoutOutlined />,
      onClick: handleLogout,
    },
  ];

  return (
    <AntHeader className={styles.header}>
      <div className={styles.headerLeft}>
        <DatabaseOutlined className={styles.logo} />
        <span className={styles.title}>{t('header.title')}</span>
        {isVerified && (
          <>
            <Divider type="vertical" className={styles.divider} />
            <SpaceSelector />
          </>
        )}
      </div>

      <div className={styles.headerRight}>
        <Space size="large">
          <LanguageSwitcher />
          <Badge
            status={isVerified ? 'success' : 'error'}
            text={
              <span className={styles.statusText}>
                {isVerified ? t('common.connected') : t('common.disconnected')}
              </span>
            }
          />

          {isVerified && (
            <>
              <Space size="small" className={styles.connectionInfo}>
                <UserOutlined />
                <span>{connectionInfo.username}</span>
              </Space>

              <Dropdown menu={{ items: menuItems }} placement="bottomRight">
                <Button
                  type="text"
                  icon={<DisconnectOutlined />}
                  loading={isLoading}
                  className={styles.disconnectBtn}
                >
                  {t('common.logout')}
                </Button>
              </Dropdown>
            </>
          )}
        </Space>
      </div>
    </AntHeader>
  );
};

export default Header;

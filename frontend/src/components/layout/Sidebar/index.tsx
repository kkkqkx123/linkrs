import React from 'react';
import { Layout, Menu } from 'antd';
import { useLocation, useNavigate } from 'react-router-dom';
import {
  ConsoleSqlOutlined,
  DatabaseOutlined,
  ApartmentOutlined,
  TableOutlined,
  ClusterOutlined,
  TagsOutlined,
  ShareAltOutlined,
  FileSearchOutlined,
  EyeOutlined,
  BarChartOutlined,
} from '@ant-design/icons';
import type { MenuProps } from 'antd';
import styles from './index.module.less';

const { Sider } = Layout;

type MenuItem = Required<MenuProps>['items'][number];

const Sidebar: React.FC = () => {
  const location = useLocation();
  const navigate = useNavigate();

  const menuItems: MenuItem[] = [
    {
      key: '/console',
      icon: <ConsoleSqlOutlined />,
      label: 'Console',
      onClick: () => navigate('/console'),
    },
    {
      key: '/schema',
      icon: <DatabaseOutlined />,
      label: 'Schema',
      children: [
        {
          key: '/schema/spaces',
          icon: <ClusterOutlined />,
          label: 'Spaces',
          onClick: () => navigate('/schema/spaces'),
        },
        {
          key: '/schema/tags',
          icon: <TagsOutlined />,
          label: 'Tags',
          onClick: () => navigate('/schema/tags'),
        },
        {
          key: '/schema/edges',
          icon: <ShareAltOutlined />,
          label: 'Edges',
          onClick: () => navigate('/schema/edges'),
        },
        {
          key: '/schema/indexes',
          icon: <FileSearchOutlined />,
          label: 'Indexes',
          onClick: () => navigate('/schema/indexes'),
        },
        {
          key: '/schema/visualization',
          icon: <EyeOutlined />,
          label: 'Visualization',
          onClick: () => navigate('/schema/visualization'),
        },
        {
          key: '/schema/stats',
          icon: <BarChartOutlined />,
          label: 'Statistics',
          onClick: () => navigate('/schema/stats'),
        },
      ],
    },
    {
      key: '/graph',
      icon: <ApartmentOutlined />,
      label: 'Graph',
      onClick: () => navigate('/graph'),
    },
    {
      key: '/data-browser',
      icon: <TableOutlined />,
      label: 'Data Browser',
      onClick: () => navigate('/data-browser'),
    },
  ];

  const getSelectedKey = () => {
    const path = location.pathname;
    if (path.startsWith('/console')) return '/console';
    if (path.startsWith('/schema/spaces')) return '/schema/spaces';
    if (path.startsWith('/schema/tags')) return '/schema/tags';
    if (path.startsWith('/schema/edges')) return '/schema/edges';
    if (path.startsWith('/schema/indexes')) return '/schema/indexes';
    if (path.startsWith('/schema/visualization')) return '/schema/visualization';
    if (path.startsWith('/schema/stats')) return '/schema/stats';
    if (path.startsWith('/schema')) return '/schema';
    if (path.startsWith('/graph')) return '/graph';
    if (path.startsWith('/data-browser')) return '/data-browser';
    return path;
  };

  const getOpenKeys = () => {
    const path = location.pathname;
    if (path.startsWith('/schema')) return ['schema'];
    return [];
  };

  return (
    <Sider className={styles.sider} width={240} theme="light">
      <Menu
        mode="inline"
        selectedKeys={[getSelectedKey()]}
        defaultOpenKeys={getOpenKeys()}
        items={menuItems}
        className={styles.menu}
      />
    </Sider>
  );
};

export default Sidebar;

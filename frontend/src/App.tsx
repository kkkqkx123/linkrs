import React from 'react';
import { RouterProvider } from 'react-router-dom';
import { ConfigProvider } from 'antd';
import router from '@/config/routes';
import themeConfig from '@/config/theme';
import './styles/global.less';

const App: React.FC = () => {
  return (
    <ConfigProvider theme={themeConfig}>
      <RouterProvider router={router} />
    </ConfigProvider>
  );
};

export default App;

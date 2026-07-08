import React from 'react';
import { Button, Dropdown } from 'antd';
import { GlobalOutlined } from '@ant-design/icons';
import { useTranslation } from 'react-i18next';

const LanguageSwitcher: React.FC = () => {
  const { i18n } = useTranslation();

  const changeLanguage = (lng: string) => {
    i18n.changeLanguage(lng);
    localStorage.setItem('graphdb_language', lng);
  };

  const menuItems = [
    {
      key: 'en',
      label: 'English',
      onClick: () => changeLanguage('en'),
    },
    {
      key: 'zh',
      label: '中文',
      onClick: () => changeLanguage('zh'),
    },
  ];

  return (
    <Dropdown menu={{ items: menuItems }} placement="bottomRight">
      <Button type="text" icon={<GlobalOutlined />}>
        {i18n.language === 'zh' ? '中文' : 'English'}
      </Button>
    </Dropdown>
  );
};

export default LanguageSwitcher;

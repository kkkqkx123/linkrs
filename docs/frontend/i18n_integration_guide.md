# 前端 i18n 集成指南

## 当前项目状态分析

### 已安装的依赖
- `i18next` (v26.0.1) - i18n 核心库
- `react-i18next` (v17.0.1) - React 集成

### 当前存在的问题
1. 项目中没有任何 i18n 配置文件或初始化代码
2. 所有 UI 文本都是硬编码的字符串（如 "Login"、"Username"、"Query Console" 等）
3. 没有语言切换功能

### 硬编码字符串分布
通过代码扫描，发现以下文件包含大量硬编码字符串：

| 文件路径 | 主要字符串 |
|---------|-----------|
| `src/pages/Login/index.tsx` | "GraphDB Studio", "Username", "Password", "Remember me", "Login", "Logged in successfully", "Login failed" |
| `src/components/layout/Header/index.tsx` | "GraphDB Studio", "Connected", "Disconnected", "Logout" |
| `src/pages/Console/index.tsx` | "Query Console", "Executing query..." |
| `src/config/routes.tsx` | 路由相关文本 |

---

## i18n 集成方案

### 1. 创建 i18n 配置文件

**文件路径**: `src/i18n/index.ts`

```typescript
import i18n from 'i18next';
import { initReactI18next } from 'react-i18next';
import en from './locales/en.json';
import zh from './locales/zh.json';

const resources = {
  en: {
    translation: en,
  },
  zh: {
    translation: zh,
  },
};

i18n
  .use(initReactI18next)
  .init({
    resources,
    lng: localStorage.getItem('graphdb_language') || 'en',
    fallbackLng: 'en',
    interpolation: {
      escapeValue: false,
    },
  });

export default i18n;
```

### 2. 创建语言文件

#### 英文语言文件

**文件路径**: `src/i18n/locales/en.json`

```json
{
  "common": {
    "login": "Login",
    "logout": "Logout",
    "username": "Username",
    "password": "Password",
    "rememberMe": "Remember me",
    "submit": "Submit",
    "cancel": "Cancel",
    "save": "Save",
    "delete": "Delete",
    "edit": "Edit",
    "create": "Create",
    "search": "Search",
    "loading": "Loading...",
    "error": "Error",
    "success": "Success",
    "confirm": "Confirm",
    "back": "Back",
    "close": "Close",
    "refresh": "Refresh",
    "settings": "Settings",
    "connected": "Connected",
    "disconnected": "Disconnected"
  },
  "login": {
    "title": "GraphDB Studio",
    "usernamePlaceholder": "Enter username",
    "passwordPlaceholder": "Enter password",
    "usernameRequired": "Please enter username",
    "passwordRequired": "Please enter password",
    "loginSuccess": "Logged in successfully",
    "loginFailed": "Login failed"
  },
  "header": {
    "title": "GraphDB Studio"
  },
  "console": {
    "title": "Query Console",
    "executing": "Executing query...",
    "history": "History",
    "favorites": "Favorites",
    "saveFavorite": "Save Favorite"
  },
  "navigation": {
    "home": "Home",
    "console": "Console",
    "schema": "Schema",
    "graph": "Graph",
    "dataBrowser": "Data Browser"
  }
}
```

#### 中文语言文件

**文件路径**: `src/i18n/locales/zh.json`

```json
{
  "common": {
    "login": "登录",
    "logout": "退出登录",
    "username": "用户名",
    "password": "密码",
    "rememberMe": "记住我",
    "submit": "提交",
    "cancel": "取消",
    "save": "保存",
    "delete": "删除",
    "edit": "编辑",
    "create": "创建",
    "search": "搜索",
    "loading": "加载中...",
    "error": "错误",
    "success": "成功",
    "confirm": "确认",
    "back": "返回",
    "close": "关闭",
    "refresh": "刷新",
    "settings": "设置",
    "connected": "已连接",
    "disconnected": "未连接"
  },
  "login": {
    "title": "GraphDB Studio",
    "usernamePlaceholder": "请输入用户名",
    "passwordPlaceholder": "请输入密码",
    "usernameRequired": "请输入用户名",
    "passwordRequired": "请输入密码",
    "loginSuccess": "登录成功",
    "loginFailed": "登录失败"
  },
  "header": {
    "title": "GraphDB Studio"
  },
  "console": {
    "title": "查询控制台",
    "executing": "正在执行查询...",
    "history": "历史记录",
    "favorites": "收藏夹",
    "saveFavorite": "保存收藏"
  },
  "navigation": {
    "home": "首页",
    "console": "控制台",
    "schema": "模式",
    "graph": "图",
    "dataBrowser": "数据浏览器"
  }
}
```

### 3. 初始化 i18n

**修改文件**: `src/main.tsx`

```typescript
import { StrictMode } from 'react';
import { createRoot } from 'react-dom/client';
import './i18n'; // 导入 i18n 配置
import './index.css';
import App from './App.tsx';

createRoot(document.getElementById('root')!).render(
  <StrictMode>
    <App />
  </StrictMode>,
);
```

### 4. 组件中使用示例

#### Login 页面示例

**文件路径**: `src/pages/Login/index.tsx`

```typescript
import React, { useEffect, useCallback } from 'react';
import { Form, Input, Button, Card, Checkbox, Spin, message } from 'antd';
import { useNavigate } from 'react-router-dom';
import { useTranslation } from 'react-i18next';
import { useConnectionStore } from '@/stores/connection';
import styles from './index.module.less';

const Login: React.FC = () => {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const { login, isLoading, loadSavedConnection } = useConnectionStore();
  const [form] = Form.useForm();

  useEffect(() => {
    loadSavedConnection();
    
    const savedConnection = localStorage.getItem('graphdb_connection');
    if (savedConnection) {
      try {
        const connectionInfo = JSON.parse(savedConnection);
        form.setFieldsValue({
          username: connectionInfo.username,
          password: connectionInfo.password || '',
          rememberMe: true,
        });
      } catch (e) {
        console.error('Failed to parse saved connection', e);
      }
    }
  }, [form, loadSavedConnection]);

  const handleSubmit = async (values: {
    username: string;
    password: string;
    rememberMe: boolean;
  }) => {
    const { username, password, rememberMe } = values;
    
    try {
      await login(username, password, rememberMe);
      message.success(t('login.loginSuccess'));
      navigate('/');
    } catch (err: unknown) {
      const errorMessage = err instanceof Error ? err.message : t('login.loginFailed');
      message.error(errorMessage);
    }
  };

  const handleRememberMeChange = useCallback((checked: boolean) => {
    console.log('Remember me:', checked);
  }, []);

  return (
    <div className={styles.loginPage}>
      <Card className={styles.loginCard} title={t('login.title')}>
        <Spin spinning={isLoading}>
          <Form
            form={form}
            name="login"
            onFinish={handleSubmit}
            layout="vertical"
            initialValues={{
              username: 'root',
              rememberMe: false,
            }}
          >
            <Form.Item
              name="username"
              label={t('common.username')}
              rules={[{ required: true, message: t('login.usernameRequired') }]}
            >
              <Input placeholder={t('login.usernamePlaceholder')} />
            </Form.Item>

            <Form.Item
              name="password"
              label={t('common.password')}
              rules={[{ required: true, message: t('login.passwordRequired') }]}
            >
              <Input.Password placeholder={t('login.passwordPlaceholder')} />
            </Form.Item>

            <Form.Item name="rememberMe" valuePropName="checked">
              <Checkbox onChange={(e) => handleRememberMeChange(e.target.checked)}>
                {t('common.rememberMe')}
              </Checkbox>
            </Form.Item>

            <Form.Item>
              <Button type="primary" htmlType="submit" block loading={isLoading}>
                {t('common.login')}
              </Button>
            </Form.Item>
          </Form>
        </Spin>
      </Card>
    </div>
  );
};

export default Login;
```

#### Header 组件示例

**文件路径**: `src/components/layout/Header/index.tsx`

```typescript
import React from 'react';
import { Layout, Button, Space, Badge, Dropdown, Divider } from 'antd';
import { DatabaseOutlined, LogoutOutlined, UserOutlined, DisconnectOutlined } from '@ant-design/icons';
import { useNavigate } from 'react-router-dom';
import { useTranslation } from 'react-i18next';
import { useConnectionStore } from '@/stores/connection';
import SpaceSelector from '@/components/business/SpaceSelector';
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
```

### 5. 语言切换组件

**文件路径**: `src/components/common/LanguageSwitcher/index.tsx`

```typescript
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
```

### 6. 在 Header 中集成语言切换器

在 `src/components/layout/Header/index.tsx` 的 `headerRight` 部分添加：

```typescript
import LanguageSwitcher from '@/components/common/LanguageSwitcher';

// 在 headerRight 的 Space 组件中添加
<Space size="large">
  <LanguageSwitcher />
  {/* 其他内容 */}
</Space>
```

---

## 改进对比

| 方面 | 改进前 | 改进后 |
|------|--------|--------|
| 字符串管理 | 硬编码在组件中 | 集中管理在 JSON 文件 |
| 多语言支持 | 单一语言 | 支持中英文切换 |
| 维护性 | 修改需改动多处 | 修改语言文件即可 |
| 扩展性 | 难以添加新语言 | 轻松添加新语言包 |
| 用户体验 | 无语言切换 | 提供语言切换功能 |

---

## 最佳实践建议

1. **命名规范**：使用 `namespace.key` 格式（如 `login.title`、`common.submit`）
2. **分类组织**：按功能模块组织翻译键（common、login、console 等）
3. **默认值**：为所有字符串提供英文默认值
4. **插值支持**：使用 `t('key', { name: value })` 支持动态内容
5. **复数支持**：使用 `t('key', { count: 5 })` 支持复数形式
6. **语言检测**：可以添加 `i18next-browser-languagedetector` 自动检测用户语言

---

## 待迁移文件清单

以下文件需要逐步迁移到 i18n：

- [ ] `src/pages/Login/index.tsx`
- [ ] `src/components/layout/Header/index.tsx`
- [ ] `src/components/layout/Sidebar/index.tsx`
- [ ] `src/pages/Console/index.tsx`
- [ ] `src/pages/Console/components/QueryEditor/index.tsx`
- [ ] `src/pages/Console/components/OutputBox/index.tsx`
- [ ] `src/pages/Console/components/HistoryPanel/index.tsx`
- [ ] `src/pages/Console/components/FavoritePanel/index.tsx`
- [ ] `src/pages/Schema/index.tsx`
- [ ] `src/pages/Schema/SpaceList/index.tsx`
- [ ] `src/pages/Schema/TagList/index.tsx`
- [ ] `src/pages/Schema/EdgeList/index.tsx`
- [ ] `src/pages/Schema/IndexList/index.tsx`
- [ ] `src/pages/Graph/index.tsx`
- [ ] `src/pages/DataBrowser/index.tsx`
- [ ] `src/components/business/DetailPanel/index.tsx`
- [ ] `src/components/business/GraphToolbar/index.tsx`
- [ ] `src/components/business/StylePanel/index.tsx`
- [ ] `src/components/business/SpaceSelector/index.tsx`
- [ ] `src/components/common/LoadingFallback/index.tsx`
- [ ] `src/components/common/LoadingScreen/index.tsx`
- [ ] `src/hooks/useHealthCheck.ts`

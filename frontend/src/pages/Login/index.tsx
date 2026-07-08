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

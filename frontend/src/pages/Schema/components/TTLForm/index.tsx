import React, { useState } from 'react';
import { Form, Select, InputNumber, Switch, Tooltip, Alert } from 'antd';
import { InfoCircleOutlined } from '@ant-design/icons';
import { useTranslation } from 'react-i18next';
import type { FormInstance } from 'antd/es/form';
import type { PropertyDef } from '@/types/schema';
import styles from './index.module.less';

interface TTLFormProps {
  form: FormInstance;
  properties: PropertyDef[];
}

const TTL_SUPPORTED_TYPES = ['INT64', 'TIMESTAMP', 'DATETIME'];

const TTLForm: React.FC<TTLFormProps> = ({ form, properties }) => {
  const { t } = useTranslation();
  const [enabled, setEnabled] = useState(false);
  const [duration, setDuration] = useState<number | null>(null);

  const ttlEligibleProperties = properties.filter((p) =>
    TTL_SUPPORTED_TYPES.includes(p.data_type)
  );

  const handleEnableChange = (checked: boolean) => {
    setEnabled(checked);
    if (!checked) {
      form.setFieldsValue({
        ttlCol: undefined,
        ttlDuration: undefined,
      });
    }
  };

  const formatDuration = (seconds: number | null): string => {
    if (!seconds) return '';
    if (seconds < 60) return `${seconds} ${t('schema.seconds')}`;
    if (seconds < 3600) return `${Math.floor(seconds / 60)} ${t('schema.minutes')}`;
    if (seconds < 86400) return `${Math.floor(seconds / 3600)} ${t('schema.hours')}`;
    return `${Math.floor(seconds / 86400)} ${t('schema.days')}`;
  };

  return (
    <div className={styles.ttlForm}>
      <div className={styles.header}>
        <h4>{t('schema.ttlConfiguration')}</h4>
        <Switch
          checked={enabled}
          onChange={handleEnableChange}
          checkedChildren={t('common.enabled')}
          unCheckedChildren={t('common.disabled')}
        />
      </div>

      {enabled && (
        <>
          {ttlEligibleProperties.length === 0 ? (
            <Alert
              type="warning"
              message={t('schema.noEligibleProperties')}
              description={t('schema.ttlPropertyRequirement')}
              showIcon
            />
          ) : (
            <>
              <Form.Item
                name="ttlCol"
                label={t('schema.ttlCol')}
                rules={[{ required: true, message: t('schema.ttlColRequired') }]}
              >
                <Select placeholder={t('schema.selectTtlCol')}>
                  {ttlEligibleProperties.map((prop) => (
                    <Select.Option key={prop.name} value={prop.name}>
                      {prop.name} ({prop.data_type})
                    </Select.Option>
                  ))}
                </Select>
              </Form.Item>

              <Form.Item
                name="ttlDuration"
                label={
                  <span>
                    {t('schema.ttlDuration')}
                    <Tooltip title={t('schema.ttlDurationTooltip')}>
                      <InfoCircleOutlined style={{ marginLeft: 8 }} />
                    </Tooltip>
                  </span>
                }
                rules={[{ required: true, message: t('schema.ttlDurationRequired') }]}
              >
                <InputNumber
                  min={1}
                  style={{ width: '100%' }}
                  placeholder={t('schema.ttlDurationPlaceholder')}
                  onChange={setDuration}
                />
              </Form.Item>

              {duration && (
                <div className={styles.durationHint}>
                  = {formatDuration(duration)}
                </div>
              )}

              <Alert
                type="info"
                message={t('schema.ttlBehaviorTitle')}
                description={t('schema.ttlBehaviorDescription')}
                showIcon
              />
            </>
          )}
        </>
      )}
    </div>
  );
};

export default TTLForm;

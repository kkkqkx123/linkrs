import React, { useEffect, useState, useCallback } from 'react';
import { Modal, Form, Input, Button, Table, Space, message, Select, Checkbox, Typography } from 'antd';
import { PlusOutlined, DeleteOutlined, EditOutlined } from '@ant-design/icons';
import { useSchemaStore } from '@/stores/schema';
import type { PropertyDef, UpdateTagParams, UpdateEdgeTypeParams } from '@/types/schema';
import styles from './index.module.less';

const { Text } = Typography;

const DATA_TYPES = ['STRING', 'INT64', 'DOUBLE', 'BOOL', 'DATETIME', 'DATE', 'TIME', 'TIMESTAMP'];

interface EditModalProps {
  visible: boolean;
  type: 'TAG' | 'EDGE';
  name: string;
  space: string;
  initialProperties: PropertyDef[];
  onCancel: () => void;
  onSuccess: () => void;
}

type PropertyStatus = 'existing' | 'added' | 'modified' | 'deleted';

interface PropertyItem extends PropertyDef {
  status: PropertyStatus;
  originalName?: string;
}

const EditModal: React.FC<EditModalProps> = ({
  visible,
  type,
  name,
  space,
  initialProperties,
  onCancel,
  onSuccess,
}) => {
  const [form] = Form.useForm();
  const [loading, setLoading] = useState(false);
  const [properties, setProperties] = useState<PropertyItem[]>([]);
  const [gql, setGql] = useState('');

  const { updateTag, updateEdgeType } = useSchemaStore();

  // Generate ALTER GQL statement
  const generateAlterGQL = useCallback((props: PropertyItem[]): string => {
    const statements: string[] = [];

    // Handle added properties
    const addedProps = props.filter((p) => p.status === 'added');
    if (addedProps.length > 0) {
      const addStr = addedProps
        .map((p) => `${p.name} ${p.data_type}${p.default_value ? ` DEFAULT ${p.default_value}` : ''}`)
        .join(', ');
      statements.push(`ADD (${addStr})`);
    }

    // Handle dropped properties
    const droppedProps = props.filter((p) => p.status === 'deleted');
    if (droppedProps.length > 0) {
      const dropStr = droppedProps.map((p) => p.originalName || p.name).join(', ');
      statements.push(`DROP (${dropStr})`);
    }

    // Handle modified properties (name change)
    const modifiedProps = props.filter((p) => p.status === 'modified');
    if (modifiedProps.length > 0) {
      modifiedProps.forEach((p) => {
        statements.push(
          `CHANGE ${p.originalName} ${p.name} ${p.data_type}${p.default_value ? ` DEFAULT ${p.default_value}` : ''}`
        );
      });
    }

    if (statements.length === 0) {
      return '-- No changes to apply';
    }

    return `ALTER ${type} ${name} ${statements.join(' ')};`;
  }, [type, name]);

  const handleAddProperty = () => {
    const newProp: PropertyItem = {
      name: '',
      data_type: 'STRING',
      nullable: true,
      status: 'added',
    };
    const newProperties = [...properties, newProp];
    setProperties(newProperties);
    form.setFieldsValue({ properties: newProperties });
  };

  const handleDeleteProperty = (index: number) => {
    const prop = properties[index];
    let newProperties: PropertyItem[];

    if (prop.status === 'existing' || prop.status === 'modified') {
      // Mark as deleted instead of removing
      newProperties = properties.map((p, i) =>
        i === index ? { ...p, status: 'deleted' as PropertyStatus } : p
      );
    } else {
      // Remove added properties completely
      newProperties = properties.filter((_, i) => i !== index);
    }

    setProperties(newProperties);
    form.setFieldsValue({ properties: newProperties });
  };

  const handlePropertyChange = (
    index: number,
    field: keyof PropertyDef,
    value: string | boolean
  ) => {
    const newProperties = [...properties];
    const prop = newProperties[index];

    // Update the field
    (prop as unknown as Record<string, unknown>)[field] = value;

    // Mark as modified if it was existing
    if (prop.status === 'existing') {
      prop.status = 'modified';
    }

    setProperties(newProperties);
  };

  // Initialize properties when modal opens
  useEffect(() => {
    if (visible && initialProperties) {
      const propsWithStatus: PropertyItem[] = initialProperties.map((p) => ({
        ...p,
        status: 'existing',
        originalName: p.name,
      }));
      setProperties(propsWithStatus);
      form.setFieldsValue({ properties: propsWithStatus });
    }
  }, [visible, initialProperties, form]);

  // Update GQL preview when properties change
  useEffect(() => {
    if (properties.length > 0) {
      const generated = generateAlterGQL(properties);
      setGql(generated);
    }
  }, [properties, generateAlterGQL]);

  const handleSubmit = async () => {
    try {
      // Filter out deleted properties for validation
      const visibleProps = properties.filter((p) => p.status !== 'deleted');

      // Validate property names
      for (const prop of visibleProps) {
        if (!prop.name.trim()) {
          message.error('Property name cannot be empty');
          return;
        }
        if (!/^[a-zA-Z][a-zA-Z0-9_]*$/.test(prop.name)) {
          message.error(`Invalid property name: ${prop.name}. Must start with letter, alphanumeric and underscores only`);
          return;
        }
      }

      // Check for duplicate names
      const names = visibleProps.map((p) => p.name);
      if (new Set(names).size !== names.length) {
        message.error('Duplicate property names are not allowed');
        return;
      }

      setLoading(true);

      const addProperties = properties
        .filter((p) => p.status === 'added')
        .map((p) => ({
          name: p.name,
          data_type: p.data_type,
          default_value: p.default_value,
          nullable: p.nullable,
        }));

      const dropProperties = properties
        .filter((p) => p.status === 'deleted')
        .map((p) => p.originalName || p.name);

      if (type === 'TAG') {
        const params: UpdateTagParams = {
          add_properties: addProperties.length > 0 ? addProperties : undefined,
          drop_properties: dropProperties.length > 0 ? dropProperties : undefined,
        };
        await updateTag(space, name, params);
      } else {
        const params: UpdateEdgeTypeParams = {
          add_properties: addProperties.length > 0 ? addProperties : undefined,
          drop_properties: dropProperties.length > 0 ? dropProperties : undefined,
        };
        await updateEdgeType(space, name, params);
      }

      message.success(`${type} "${name}" updated successfully`);
      onSuccess();
    } catch (err: unknown) {
      const errorMessage = err instanceof Error ? err.message : 'Failed to update schema';
      message.error(errorMessage);
    } finally {
      setLoading(false);
    }
  };

  const visibleProperties = properties.filter((p) => p.status !== 'deleted');

  const columns = [
    {
      title: 'Name',
      key: 'name',
      render: (_: unknown, __: unknown, index: number) => {
        const prop = visibleProperties[index];
        const actualIndex = properties.findIndex((p) => p === prop);
        return (
          <Input
            value={prop.name}
            onChange={(e) => handlePropertyChange(actualIndex, 'name', e.target.value)}
            placeholder="Property name"
            style={{ width: 150 }}
            disabled={prop.status === 'existing'}
            status={prop.status === 'modified' ? 'warning' : undefined}
          />
        );
      },
    },
    {
      title: 'Type',
      key: 'type',
      render: (_: unknown, __: unknown, index: number) => {
        const prop = visibleProperties[index];
        const actualIndex = properties.findIndex((p) => p === prop);
        return (
          <Select
            value={prop.data_type}
            onChange={(value) => handlePropertyChange(actualIndex, 'data_type', value)}
            style={{ width: 120 }}
            options={DATA_TYPES.map((t) => ({ value: t, label: t }))}
            disabled={prop.status === 'existing'}
          />
        );
      },
    },
    {
      title: 'Default',
      key: 'default',
      render: (_: unknown, __: unknown, index: number) => {
        const prop = visibleProperties[index];
        const actualIndex = properties.findIndex((p) => p === prop);
        return (
          <Input
            value={prop.default_value || ''}
            onChange={(e) => handlePropertyChange(actualIndex, 'default_value', e.target.value)}
            placeholder="Optional"
            style={{ width: 120 }}
          />
        );
      },
    },
    {
      title: 'Nullable',
      key: 'nullable',
      render: (_: unknown, __: unknown, index: number) => {
        const prop = visibleProperties[index];
        const actualIndex = properties.findIndex((p) => p === prop);
        return (
          <Checkbox
            checked={prop.nullable}
            onChange={(e) => handlePropertyChange(actualIndex, 'nullable', e.target.checked)}
          />
        );
      },
    },
    {
      title: 'Status',
      key: 'status',
      render: (_: unknown, record: PropertyItem) => {
        const statusMap: Record<PropertyStatus, { text: string; color: string }> = {
          existing: { text: 'Existing', color: 'default' },
          added: { text: 'New', color: 'success' },
          modified: { text: 'Modified', color: 'warning' },
          deleted: { text: 'Deleted', color: 'error' },
        };
        const status = statusMap[record.status];
        return <Text type={status.color as 'secondary' | 'success' | 'warning' | 'danger'}>{status.text}</Text>;
      },
    },
    {
      title: 'Action',
      key: 'action',
      render: (_: unknown, __: unknown, index: number) => {
        const prop = visibleProperties[index];
        const actualIndex = properties.findIndex((p) => p === prop);
        return (
          <Button
            type="link"
            danger
            icon={<DeleteOutlined />}
            onClick={() => handleDeleteProperty(actualIndex)}
          >
            Delete
          </Button>
        );
      },
    },
  ];

  return (
    <Modal
      title={
        <Space>
          <EditOutlined />
          <span>
            Edit {type}: {name}
          </span>
        </Space>
      }
      open={visible}
      width={900}
      onCancel={onCancel}
      footer={[
        <Button key="cancel" onClick={onCancel}>
          Cancel
        </Button>,
        <Button key="submit" type="primary" loading={loading} onClick={handleSubmit}>
          Save Changes
        </Button>,
      ]}
    >
      <Form form={form} layout="vertical">
        <div className={styles.section}>
          <div className={styles.sectionHeader}>
            <Text strong>Properties</Text>
            <Button type="dashed" icon={<PlusOutlined />} onClick={handleAddProperty} size="small">
              Add Property
            </Button>
          </div>

          <Table
            dataSource={visibleProperties}
            columns={columns}
            pagination={false}
            size="small"
            rowKey={(record, index) => `${record.name}-${index}`}
            locale={{ emptyText: 'No properties' }}
          />
        </div>

        <div className={styles.section}>
          <Text strong>GQL Preview</Text>
          <div className={styles.gqlPreview}>
            <pre>{gql}</pre>
          </div>
        </div>
      </Form>
    </Modal>
  );
};

export default EditModal;

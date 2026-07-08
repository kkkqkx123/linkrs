import React, { useState, useCallback, useMemo } from 'react';
import { Modal, Form, Input, Select, Button, message } from 'antd';
import type { DragEndEvent } from '@dnd-kit/core';
import {
  DndContext,
  closestCenter,
  KeyboardSensor,
  PointerSensor,
  useSensor,
  useSensors,
} from '@dnd-kit/core';
import {
  arrayMove,
  SortableContext,
  sortableKeyboardCoordinates,
  verticalListSortingStrategy,
} from '@dnd-kit/sortable';
import { PlusOutlined } from '@ant-design/icons';
import { useSchemaStore } from '@/stores/schema';
import DraggableField from './DraggableField';
import FieldSelectModal from './FieldSelectModal';
import styles from './index.module.less';

const { Option } = Select;

interface CreateIndexModalProps {
  visible: boolean;
  space: string;
  onCancel: () => void;
  onSuccess: () => void;
}

interface IndexField {
  id: string;
  name: string;
  type: string;
}

const CreateIndexModal: React.FC<CreateIndexModalProps> = ({
  visible,
  space,
  onCancel,
  onSuccess,
}) => {
  const [form] = Form.useForm();
  const { tags, edgeTypes, createIndex } = useSchemaStore();

  const [indexType, setIndexType] = useState<'TAG' | 'EDGE'>('TAG');
  const [selectedEntity, setSelectedEntity] = useState<string>('');
  const [fields, setFields] = useState<IndexField[]>([]);
  const [fieldSelectVisible, setFieldSelectVisible] = useState(false);

  const sensors = useSensors(
    useSensor(PointerSensor),
    useSensor(KeyboardSensor, {
      coordinateGetter: sortableKeyboardCoordinates,
    })
  );

  // Generate GQL preview using useMemo instead of useEffect + setState
  const gqlPreview = useMemo(() => {
    const values = form.getFieldsValue();
    if (!values.name || !selectedEntity || fields.length === 0) {
      return '';
    }

    const fieldList = fields.map((f) => f.name).join(', ');
    const entityType = indexType === 'TAG' ? 'TAG' : 'EDGE';

    return `CREATE ${entityType} INDEX ${values.name} ON ${selectedEntity}(${fieldList});`;
  }, [form, indexType, selectedEntity, fields]);

  // Handle modal open/close with proper cleanup
  const handleCancel = () => {
    form.resetFields();
    setIndexType('TAG');
    setSelectedEntity('');
    setFields([]);
    onCancel();
  };

  // Drag end handler
  const handleDragEnd = (event: DragEndEvent) => {
    const { active, over } = event;

    if (over && active.id !== over.id) {
      setFields((items) => {
        const oldIndex = items.findIndex((i) => i.id === active.id);
        const newIndex = items.findIndex((i) => i.id === over.id);
        return arrayMove(items, oldIndex, newIndex);
      });
    }
  };

  // Get entity properties
  const getEntityProperties = useCallback((entityType: 'TAG' | 'EDGE', entityName: string) => {
    if (entityType === 'TAG') {
      const tag = tags.find((t) => t.name === entityName);
      return tag?.properties || [];
    } else {
      const edge = edgeTypes.find((e) => e.name === entityName);
      return edge?.properties || [];
    }
  }, [tags, edgeTypes]);

  // Add fields
  const handleAddFields = (selectedFieldNames: string[]) => {
    const entityProps = getEntityProperties(indexType, selectedEntity);

    const newFields = selectedFieldNames.map((name) => {
      const prop = entityProps.find((p) => p.name === name);
      return {
        id: `${name}_${Date.now()}_${Math.random().toString(36).substr(2, 9)}`,
        name,
        type: prop?.data_type || 'STRING',
      };
    });

    setFields((prev) => [...prev, ...newFields]);
    setFieldSelectVisible(false);
  };

  // Remove field
  const handleRemoveField = (id: string) => {
    setFields((prev) => prev.filter((f) => f.id !== id));
  };

  // Submit create
  const handleSubmit = async () => {
    try {
      await form.validateFields();

      if (fields.length === 0) {
        message.error('Please select at least one field');
        return;
      }

      const values = form.getFieldsValue();
      await createIndex(space, {
        name: values.name,
        index_type: indexType,
        entity_type: indexType,
        entity_name: selectedEntity,
        fields: fields.map((f) => f.name),
      });

      message.success(`Index "${values.name}" created successfully`);
      onSuccess();
    } catch (err: unknown) {
      const errorMessage = err instanceof Error ? err.message : 'Failed to create index';
      message.error(errorMessage);
    }
  };

  const entityOptions = indexType === 'TAG' ? tags : edgeTypes;

  return (
    <>
      <Modal
        title="Create Index"
        open={visible}
        width={600}
        onOk={handleSubmit}
        onCancel={handleCancel}
      >
        <Form
          form={form}
          layout="vertical"
        >
          <Form.Item
            name="name"
            label="Index Name"
            rules={[
              { required: true, message: 'Please enter index name' },
              {
                pattern: /^[a-zA-Z][a-zA-Z0-9_]*$/,
                message: 'Must start with letter, alphanumeric and underscores only',
              },
            ]}
          >
            <Input placeholder="e.g., idx_person_name" />
          </Form.Item>

          <Form.Item label="Index Type">
            <Select
              value={indexType}
              onChange={(value: 'TAG' | 'EDGE') => {
                setIndexType(value);
                setSelectedEntity('');
                setFields([]);
              }}
            >
              <Option value="TAG">Tag</Option>
              <Option value="EDGE">Edge</Option>
            </Select>
          </Form.Item>

          <Form.Item label="Entity">
            <Select
              value={selectedEntity}
              onChange={(value) => {
                setSelectedEntity(value);
                setFields([]);
              }}
              placeholder={`Select ${indexType.toLowerCase()}`}
            >
              {entityOptions.map((e) => (
                <Option key={e.name} value={e.name}>
                  {e.name}
                </Option>
              ))}
            </Select>
          </Form.Item>

          <div className={styles.fieldsSection}>
            <label>Index Fields (drag to reorder)</label>

            <DndContext
              sensors={sensors}
              collisionDetection={closestCenter}
              onDragEnd={handleDragEnd}
            >
              <SortableContext
                items={fields.map((f) => f.id)}
                strategy={verticalListSortingStrategy}
              >
                <div className={styles.fieldsList}>
                  {fields.map((field) => (
                    <DraggableField
                      key={field.id}
                      id={field.id}
                      name={field.name}
                      type={field.type}
                      onRemove={() => handleRemoveField(field.id)}
                    />
                  ))}
                </div>
              </SortableContext>
            </DndContext>

            <Button
              type="dashed"
              icon={<PlusOutlined />}
              onClick={() => setFieldSelectVisible(true)}
              disabled={!selectedEntity}
              block
            >
              Add Field
            </Button>
          </div>

          {gqlPreview && (
            <div className={styles.gqlPreview}>
              <label>GQL Preview:</label>
              <pre>{gqlPreview}</pre>
            </div>
          )}
        </Form>
      </Modal>

      <FieldSelectModal
        visible={fieldSelectVisible}
        space={space}
        entityType={indexType}
        entityName={selectedEntity}
        selectedFields={fields.map((f) => f.name)}
        onConfirm={handleAddFields}
        onCancel={() => setFieldSelectVisible(false)}
      />
    </>
  );
};

export default CreateIndexModal;

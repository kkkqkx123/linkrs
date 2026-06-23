import React, { useState, useEffect, useCallback } from 'react';
import { Modal, Checkbox, List, Tag, Empty, Spin } from 'antd';
import { useSchemaStore } from '@/stores/schema';
import type { PropertyDef } from '@/types/schema';
import styles from './index.module.less';

interface FieldSelectModalProps {
  visible: boolean;
  space: string;
  entityType: 'TAG' | 'EDGE';
  entityName: string;
  selectedFields: string[];
  onConfirm: (fields: string[]) => void;
  onCancel: () => void;
}

const FieldSelectModal: React.FC<FieldSelectModalProps> = ({
  visible,
  entityType,
  entityName,
  selectedFields,
  onConfirm,
  onCancel,
}) => {
  const { tags, edgeTypes } = useSchemaStore();
  const [fields, setFields] = useState<PropertyDef[]>([]);
  const [selected, setSelected] = useState<string[]>(selectedFields);
  const [loading, setLoading] = useState(false);

  const loadFields = useCallback(() => {
    setLoading(true);
    try {
      let entity;
      if (entityType === 'TAG') {
        entity = tags.find((t) => t.name === entityName);
      } else {
        entity = edgeTypes.find((e) => e.name === entityName);
      }
      setFields(entity?.properties || []);
    } finally {
      setLoading(false);
    }
  }, [entityType, entityName, tags, edgeTypes]);

  useEffect(() => {
    if (visible && entityName) {
      loadFields();
    }
    setSelected(selectedFields);
  }, [visible, entityName, selectedFields, loadFields]);

  const handleToggle = (fieldName: string) => {
    setSelected((prev) =>
      prev.includes(fieldName)
        ? prev.filter((f) => f !== fieldName)
        : [...prev, fieldName]
    );
  };

  const handleConfirm = () => {
    onConfirm(selected);
  };

  return (
    <Modal
      title="Select Fields"
      open={visible}
      width={500}
      onOk={handleConfirm}
      onCancel={onCancel}
      confirmLoading={loading}
    >
      <Spin spinning={loading}>
        <div className={styles.fieldSelectModal}>
          <p className={styles.entityInfo}>
            Available fields from {entityType.toLowerCase()} <strong>{entityName}</strong>:
          </p>

          {fields.length === 0 ? (
            <Empty description="No fields available" />
          ) : (
            <List
              dataSource={fields}
              renderItem={(field) => (
                <List.Item
                  className={styles.fieldItem}
                  onClick={() => handleToggle(field.name)}
                >
                  <Checkbox
                    checked={selected.includes(field.name)}
                    onChange={() => handleToggle(field.name)}
                  />
                  <span className={styles.fieldName}>{field.name}</span>
                  <Tag>{field.data_type}</Tag>
                  {field.comment && (
                    <span className={styles.fieldComment}>{field.comment}</span>
                  )}
                </List.Item>
              )}
            />
          )}

          <div className={styles.selectedSummary}>
            Selected: {selected.length > 0 ? selected.join(', ') : 'None'}
          </div>
        </div>
      </Spin>
    </Modal>
  );
};

export default FieldSelectModal;

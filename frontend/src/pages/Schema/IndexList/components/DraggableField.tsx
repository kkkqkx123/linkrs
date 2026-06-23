import React from 'react';
import { useSortable } from '@dnd-kit/sortable';
import { CSS } from '@dnd-kit/utilities';
import { HolderOutlined, CloseOutlined } from '@ant-design/icons';
import { Tag, Button } from 'antd';
import styles from './index.module.less';

interface DraggableFieldProps {
  id: string;
  name: string;
  type: string;
  onRemove: () => void;
}

const DraggableField: React.FC<DraggableFieldProps> = ({
  id,
  name,
  type,
  onRemove,
}) => {
  const {
    attributes,
    listeners,
    setNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id });

  const style = {
    transform: CSS.Transform.toString(transform),
    transition,
    opacity: isDragging ? 0.5 : 1,
  };

  return (
    <div
      ref={setNodeRef}
      style={style}
      className={styles.draggableField}
    >
      <HolderOutlined
        className={styles.dragHandle}
        {...attributes}
        {...listeners}
      />
      <span className={styles.fieldName}>{name}</span>
      <Tag>{type}</Tag>
      <Button
        type="text"
        size="small"
        icon={<CloseOutlined />}
        onClick={onRemove}
        danger
      />
    </div>
  );
};

export default DraggableField;

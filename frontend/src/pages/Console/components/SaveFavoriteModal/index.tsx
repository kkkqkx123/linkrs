import React, { useState, useEffect } from 'react';
import {
  Modal,
  Form,
  Input,
  Typography,
  Alert,
} from 'antd';
import { StarOutlined } from '@ant-design/icons';
import { useConsoleStore } from '@/stores/console';
import styles from './index.module.less';

const { Paragraph } = Typography;

interface SaveFavoriteModalProps {
  open: boolean;
  onClose: () => void;
}

const SaveFavoriteModal: React.FC<SaveFavoriteModalProps> = ({
  open,
  onClose,
}) => {
  const [form] = Form.useForm();
  const { editorContent, addToFavorites, isFavoriteNameExists } = useConsoleStore();
  const [error, setError] = useState<string | null>(null);

  // Reset form when opened
  useEffect(() => {
    if (open) {
      form.resetFields();
      // Clear error in next tick to avoid setState in render
      const timer = setTimeout(() => setError(null), 0);
      return () => clearTimeout(timer);
    }
  }, [open, form]);

  // Handle save
  const handleSave = async () => {
    try {
      const values = await form.validateFields();
      const name = values.name.trim();

      // Check if name already exists
      if (isFavoriteNameExists(name)) {
        setError('A favorite with this name already exists');
        return;
      }

      // Add to favorites
      const result = addToFavorites(name, editorContent);

      if (result.success) {
        onClose();
      } else {
        setError(result.error || 'Failed to save favorite');
      }
    } catch {
      // Form validation failed
    }
  };

  // Handle cancel
  const handleCancel = () => {
    onClose();
  };

  // Clear error when name changes
  const handleNameChange = () => {
    setError(null);
  };

  return (
    <Modal
      title={
        <div className={styles.modalTitle}>
          <StarOutlined />
          <span>Save to Favorites</span>
        </div>
      }
      open={open}
      onOk={handleSave}
      onCancel={handleCancel}
      okText="Save"
      cancelText="Cancel"
      destroyOnClose
    >
      <Form
        form={form}
        layout="vertical"
        className={styles.form}
      >
        {error && (
          <Alert
            message={error}
            type="error"
            showIcon
            className={styles.error}
            closable
            onClose={() => setError(null)}
          />
        )}

        <Form.Item
          name="name"
          label="Favorite Name"
          rules={[
            { required: true, message: 'Please enter a name' },
            { max: 50, message: 'Name must be less than 50 characters' },
            {
              pattern: /^[a-zA-Z0-9_\-\s]+$/,
              message: 'Name can only contain letters, numbers, spaces, hyphens and underscores',
            },
          ]}
        >
          <Input
            placeholder="Enter a name for this query"
            onChange={handleNameChange}
            autoFocus
          />
        </Form.Item>

        <Form.Item label="Query Preview">
          <div className={styles.queryPreview}>
            <Paragraph
              ellipsis={{ rows: 4 }}
              className={styles.queryText}
            >
              {editorContent || 'No query content'}
            </Paragraph>
          </div>
        </Form.Item>

        <Typography.Text type="secondary" className={styles.hint}>
          This query will be saved to your favorites for quick access later.
        </Typography.Text>
      </Form>
    </Modal>
  );
};

export default SaveFavoriteModal;

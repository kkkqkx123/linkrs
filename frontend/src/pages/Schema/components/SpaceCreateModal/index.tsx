import React, { useState } from 'react';
import { Modal, Form, Input, Select, InputNumber, message } from 'antd';
import { useSchemaStore, type CreateSpaceParams } from '@/stores/schema';
import styles from './index.module.less';



interface SpaceCreateModalProps {
  visible: boolean;
  onCancel: () => void;
  onSuccess: () => void;
}

const SpaceCreateModal: React.FC<SpaceCreateModalProps> = ({
  visible,
  onCancel,
  onSuccess,
}) => {
  const [form] = Form.useForm();
  const { createSpace } = useSchemaStore();
  const [isSubmitting, setIsSubmitting] = useState(false);

  const handleSubmit = async () => {
    try {
      const values = await form.validateFields();
      setIsSubmitting(true);

      const params: CreateSpaceParams = {
        name: values.name,
        vidType: values.vidType,
        partitionNum: values.partitionNum,
        replicaFactor: values.replicaFactor,
      };

      await createSpace(params);
      message.success(`Space "${values.name}" created successfully`);
      form.resetFields();
      onSuccess();
    } catch (err: unknown) {
      const errorMessage = err instanceof Error ? err.message : 'Failed to create space';
      message.error(errorMessage);
    } finally {
      setIsSubmitting(false);
    }
  };

  const handleCancel = () => {
    form.resetFields();
    onCancel();
  };

  return (
    <Modal
      title="Create New Space"
      open={visible}
      onOk={handleSubmit}
      onCancel={handleCancel}
      confirmLoading={isSubmitting}
      okText="Create"
      cancelText="Cancel"
      width={500}
    >
      <Form
        form={form}
        layout="vertical"
        initialValues={{
          vidType: 'INT64',
          partitionNum: 100,
          replicaFactor: 1,
        }}
        className={styles.form}
      >
        <Form.Item
          label="Space Name"
          name="name"
          rules={[
            { required: true, message: 'Please input Space name' },
            {
              pattern: /^[a-zA-Z][a-zA-Z0-9_]*$/,
              message: 'Name must start with a letter and contain only alphanumeric characters and underscores',
            },
            { max: 64, message: 'Name must be less than 64 characters' },
          ]}
          extra="Space name must start with a letter and can contain letters, numbers, and underscores."
        >
          <Input placeholder="Enter space name" />
        </Form.Item>

        <Form.Item
          label="Vid Type"
          name="vidType"
          rules={[{ required: true, message: 'Please select Vid Type' }]}
          extra="Vid Type determines the data type of vertex IDs. INT64 is recommended for numeric IDs, FIXED_STRING(32) for string IDs."
        >
          <Select
            placeholder="Select Vid Type"
            options={[
              { label: 'INT64', value: 'INT64' },
              { label: 'FIXED_STRING(32)', value: 'FIXED_STRING(32)' },
            ]}
          />
        </Form.Item>

        <Form.Item
          label="Partition Number"
          name="partitionNum"
          rules={[
            { required: true, message: 'Please input partition number' },
            { type: 'number', min: 1, message: 'Partition number must be at least 1' },
          ]}
          extra="Number of partitions for data distribution. Default is 100."
        >
          <InputNumber min={1} style={{ width: '100%' }} />
        </Form.Item>

        <Form.Item
          label="Replica Factor"
          name="replicaFactor"
          rules={[
            { required: true, message: 'Please input replica factor' },
            { type: 'number', min: 1, message: 'Replica factor must be at least 1' },
          ]}
          extra="Number of replicas for data reliability. Default is 1."
        >
          <InputNumber min={1} style={{ width: '100%' }} />
        </Form.Item>
      </Form>
    </Modal>
  );
};

export default SpaceCreateModal;

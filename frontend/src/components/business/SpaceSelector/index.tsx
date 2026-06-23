import React, { useEffect } from 'react';
import { Select, Space, Tooltip } from 'antd';
import { DatabaseOutlined, ReloadOutlined } from '@ant-design/icons';
import { useSchemaStore } from '@/stores/schema';
import styles from './index.module.less';

const { Option } = Select;

interface SpaceSelectorProps {
  showRefresh?: boolean;
}

const SpaceSelector: React.FC<SpaceSelectorProps> = ({ showRefresh = true }) => {
  const {
    spaces,
    currentSpace,
    isLoadingSpaces,
    fetchSpaces,
    setCurrentSpace,
  } = useSchemaStore();

  useEffect(() => {
    // Fetch spaces on mount if not already loaded
    if (spaces.length === 0) {
      fetchSpaces();
    }
  }, [spaces.length, fetchSpaces]);

  const handleChange = (value: string) => {
    setCurrentSpace(value);
  };

  const handleRefresh = () => {
    fetchSpaces();
  };

  return (
    <Space className={styles.container}>
      <DatabaseOutlined className={styles.icon} />
      <Select
        value={currentSpace}
        onChange={handleChange}
        loading={isLoadingSpaces}
        placeholder="Select Space"
        className={styles.select}
        dropdownMatchSelectWidth={false}
        showSearch
        filterOption={(input, option) =>
          (option?.children as unknown as string)
            ?.toLowerCase()
            .includes(input.toLowerCase())
        }
      >
        {spaces.map((space) => (
          <Option key={space.id} value={space.name}>
            {space.name}
          </Option>
        ))}
      </Select>
      {showRefresh && (
        <Tooltip title="Refresh Spaces">
          <ReloadOutlined
            className={styles.refreshIcon}
            onClick={handleRefresh}
            spin={isLoadingSpaces}
          />
        </Tooltip>
      )}
    </Space>
  );
};

export default SpaceSelector;

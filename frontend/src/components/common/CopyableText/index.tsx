import React from 'react';
import { Button, Space, Tooltip } from 'antd';
import { CopyOutlined } from '@ant-design/icons';
import { copyToClipboard } from '@/utils/function';
import styles from './index.module.less';

interface CopyableTextProps {
  text: string;
  title?: string;
  showIcon?: boolean;
  maxLength?: number;
  className?: string;
}

const CopyableText: React.FC<CopyableTextProps> = ({
  text,
  title = 'Copy',
  showIcon = true,
  maxLength,
  className,
}) => {
  const handleCopy = () => {
    copyToClipboard(text);
  };

  const displayText = maxLength && text.length > maxLength
    ? `${text.slice(0, maxLength)}...`
    : text;

  return (
    <Space className={className}>
      <span className={styles.text} title={text}>
        {displayText}
      </span>
      {showIcon && (
        <Tooltip title={title}>
          <Button
            icon={<CopyOutlined />}
            size="small"
            type="text"
            onClick={handleCopy}
          />
        </Tooltip>
      )}
    </Space>
  );
};

export default CopyableText;

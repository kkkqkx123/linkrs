import React, { useState, useEffect, useCallback } from 'react';
import { Modal, Button, Spin, message } from 'antd';
import { CopyOutlined, DownloadOutlined, FileTextOutlined } from '@ant-design/icons';
import { schemaService } from '@/services/schema';
import type { DDLData } from '@/types/schema';
import styles from './index.module.less';

interface DDLExportModalProps {
  visible: boolean;
  space: string;
  onCancel: () => void;
}

const DDLExportModal: React.FC<DDLExportModalProps> = ({
  visible,
  space,
  onCancel,
}) => {
  const [loading, setLoading] = useState(false);
  const [ddl, setDDL] = useState('');
  const [generatedAt, setGeneratedAt] = useState<string>('');

  const escapeIdentifier = (name: string): string => {
    if (/^[a-zA-Z_][a-zA-Z0-9_]*$/.test(name)) {
      return name;
    }
    return `\`${name}\``;
  };

  const formatDDL = useCallback((data: DDLData): string => {
    const lines: string[] = [];

    lines.push('# Create Space');
    lines.push(data.space);
    lines.push(':sleep 20;');

    const spaceNameMatch = data.space.match(/CREATE SPACE (\w+)/);
    const spaceName = spaceNameMatch ? spaceNameMatch[1] : space;
    lines.push(`USE ${escapeIdentifier(spaceName)};`);
    lines.push('');

    if (data.tags.length > 0) {
      lines.push('# Create Tags');
      data.tags.forEach(tag => {
        lines.push(tag);
        lines.push('');
      });
    }

    if (data.edges.length > 0) {
      lines.push('# Create Edges');
      data.edges.forEach(edge => {
        lines.push(edge);
        lines.push('');
      });
    }

    if (data.indexes.length > 0) {
      lines.push('# Create Indexes');
      lines.push(':sleep 20;');
      data.indexes.forEach(index => {
        lines.push(index);
        lines.push('');
      });
    }

    return lines.join('\n');
  }, [space]);

  const fetchDDL = useCallback(async () => {
    if (!visible || !space) return;

    setLoading(true);
    try {
      const data = await schemaService.exportDDL(space);
      const formattedDDL = formatDDL(data);
      setDDL(formattedDDL);
      setGeneratedAt(new Date().toLocaleString());
    } catch (error) {
      const errorMessage = error instanceof Error ? error.message : 'Failed to fetch DDL';
      message.error(errorMessage);
    } finally {
      setLoading(false);
    }
  }, [visible, space, formatDDL]);

  useEffect(() => {
    if (visible) {
      fetchDDL();
    }
  }, [fetchDDL, visible]);

  const handleCopy = async () => {
    try {
      await navigator.clipboard.writeText(ddl);
      message.success('DDL copied to clipboard');
    } catch {
      message.error('Failed to copy');
    }
  };

  const handleDownload = () => {
    const blob = new Blob([ddl], { type: 'text/plain;charset=utf-8' });
    const url = URL.createObjectURL(blob);
    const link = document.createElement('a');
    link.href = url;
    link.download = `${space}_ddl.ngql`;
    document.body.appendChild(link);
    link.click();
    document.body.removeChild(link);
    URL.revokeObjectURL(url);
    message.success('DDL downloaded');
  };

  return (
    <Modal
      title={
        <div className={styles.modalTitle}>
          <FileTextOutlined className={styles.titleIcon} />
          DDL Export: {space}
        </div>
      }
      open={visible}
      width={800}
      onCancel={onCancel}
      footer={[
        <Button key="copy" icon={<CopyOutlined />} onClick={handleCopy} disabled={!ddl || loading}>
          Copy
        </Button>,
        <Button
          key="download"
          type="primary"
          icon={<DownloadOutlined />}
          onClick={handleDownload}
          disabled={!ddl || loading}
        >
          Download
        </Button>,
        <Button key="close" onClick={onCancel}>
          Close
        </Button>,
      ]}
    >
      <Spin spinning={loading} tip="Generating DDL...">
        <div className={styles.ddlContainer}>
          {ddl ? (
            <>
              <pre className={styles.ddlContent}>{ddl}</pre>
              <div className={styles.generatedTime}>
                Generated: {generatedAt}
              </div>
            </>
          ) : (
            <div className={styles.emptyState}>
              {!loading && 'No DDL available'}
            </div>
          )}
        </div>
      </Spin>
    </Modal>
  );
};

export default DDLExportModal;

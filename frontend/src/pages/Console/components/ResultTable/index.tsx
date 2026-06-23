import React, { useState, useMemo } from 'react';
import { Table, Empty, Typography } from 'antd';
import type { ColumnsType } from 'antd/es/table';
import type { QueryResult } from '@/types/query';
import { formatCellValue, sortRows } from '@/utils/parseData';
import styles from './index.module.less';

const { Text } = Typography;

interface ResultTableProps {
  result: QueryResult;
}

interface TableData {
  key: string;
  [key: string]: unknown;
}

const ResultTable: React.FC<ResultTableProps> = ({ result }) => {
  const [sortedColumn, setSortedColumn] = useState<string | null>(null);
  const [sortDirection, setSortDirection] = useState<'asc' | 'desc'>('asc');

  // Convert result to table data
  const tableData = useMemo(() => {
    if (!result || !result.rows) return [];

    let rows = result.rows;

    // Apply sorting if active
    if (sortedColumn && result.columns) {
      const columnIndex = result.columns.indexOf(sortedColumn);
      if (columnIndex !== -1) {
        rows = sortRows(rows, columnIndex, sortDirection);
      }
    }

    return rows.map((row, rowIndex) => {
      const rowData: TableData = { key: `row-${rowIndex}` };
      result.columns.forEach((col, colIndex) => {
        rowData[col] = row[colIndex];
      });
      return rowData;
    });
  }, [result, sortedColumn, sortDirection]);

  // Generate columns
  const columns: ColumnsType<TableData> = useMemo(() => {
    if (!result || !result.columns) return [];

    return result.columns.map((col) => ({
      title: col,
      dataIndex: col,
      key: col,
      sorter: true,
      sortOrder: sortedColumn === col ? (sortDirection === 'asc' ? 'ascend' : 'descend') : undefined,
      render: (value: unknown) => (
        <Text className={styles.cell} title={formatCellValue(value)}>
          {formatCellValue(value)}
        </Text>
      ),
      onHeaderCell: () => ({
        onClick: () => {
          if (sortedColumn === col) {
            setSortDirection(sortDirection === 'asc' ? 'desc' : 'asc');
          } else {
            setSortedColumn(col);
            setSortDirection('asc');
          }
        },
      }),
    }));
  }, [result, sortedColumn, sortDirection]);

  if (!result || !result.columns || result.columns.length === 0) {
    return (
      <Empty
        description="No data available"
        className={styles.empty}
      />
    );
  }

  return (
    <div className={styles.resultTable}>
      <Table
        columns={columns}
        dataSource={tableData}
        pagination={{
          pageSize: 100,
          showSizeChanger: false,
          showTotal: (total) => `Total ${total} rows`,
        }}
        scroll={{ x: 'max-content', y: 400 }}
        size="small"
        bordered
        className={styles.table}
      />
    </div>
  );
};

export default ResultTable;

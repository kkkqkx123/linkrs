import React from 'react';
import { Card, Select, Input, Button, Space, Radio, Row, Col } from 'antd';
import { PlusOutlined, DeleteOutlined, ClearOutlined } from '@ant-design/icons';
import { useDataBrowserStore } from '@/stores/dataBrowser';
import type { FilterOperator } from '@/types/dataBrowser';
import styles from './index.module.less';



const OPERATORS: { value: FilterOperator; label: string }[] = [
  { value: 'eq', label: '=' },
  { value: 'ne', label: '≠' },
  { value: 'gt', label: '>' },
  { value: 'lt', label: '<' },
  { value: 'ge', label: '≥' },
  { value: 'le', label: '≤' },
  { value: 'contains', label: 'Contains' },
  { value: 'startsWith', label: 'Starts with' },
  { value: 'endsWith', label: 'Ends with' },
];

interface FilterPanelProps {
  properties: string[];
}

const FilterPanel: React.FC<FilterPanelProps> = ({ properties }) => {
  const {
    filters,
    addFilterCondition,
    removeFilterCondition,
    setFilters,
    clearFilters,
  } = useDataBrowserStore();

  const handleAddCondition = () => {
    if (properties.length > 0) {
      addFilterCondition({
        property: properties[0],
        operator: 'eq',
        value: '',
      });
    }
  };

  const handleUpdateCondition = (
    index: number,
    field: 'property' | 'operator' | 'value',
    value: string | FilterOperator
  ) => {
    const newConditions = [...filters.conditions];
    newConditions[index] = { ...newConditions[index], [field]: value };
    setFilters({ ...filters, conditions: newConditions });
  };

  return (
    <Card
      title="Filter Conditions"
      size="small"
      className={styles.panel}
      extra={
        <Space>
          {filters.conditions.length > 0 && (
            <Button
              icon={<ClearOutlined />}
              size="small"
              onClick={clearFilters}
            >
              Clear
            </Button>
          )}
          <Button
            type="primary"
            icon={<PlusOutlined />}
            size="small"
            onClick={handleAddCondition}
            disabled={properties.length === 0}
          >
            Add
          </Button>
        </Space>
      }
    >
      {filters.conditions.length > 1 && (
        <div className={styles.logicSelector}>
          <Radio.Group
            value={filters.logic}
            onChange={(e) => setFilters({ ...filters, logic: e.target.value })}
            size="small"
          >
            <Radio.Button value="AND">Match All (AND)</Radio.Button>
            <Radio.Button value="OR">Match Any (OR)</Radio.Button>
          </Radio.Group>
        </div>
      )}

      <div className={styles.conditions}>
        {filters.conditions.map((condition, index) => (
          <Row key={index} gutter={8} className={styles.conditionRow}>
            <Col span={7}>
              <Select
                value={condition.property}
                onChange={(value) => handleUpdateCondition(index, 'property', value)}
                size="small"
                style={{ width: '100%' }}
                placeholder="Property"
                options={properties.map((prop) => ({
                  label: prop,
                  value: prop,
                }))}
              />
            </Col>
            <Col span={6}>
              <Select
                value={condition.operator}
                onChange={(value) => handleUpdateCondition(index, 'operator', value)}
                size="small"
                style={{ width: '100%' }}
                placeholder="Operator"
                options={OPERATORS.map((op) => ({
                  label: op.label,
                  value: op.value,
                }))}
              />
            </Col>
            <Col span={8}>
              <Input
                value={String(condition.value)}
                onChange={(e) => handleUpdateCondition(index, 'value', e.target.value)}
                size="small"
                placeholder="Value"
              />
            </Col>
            <Col span={3}>
              <Button
                icon={<DeleteOutlined />}
                size="small"
                danger
                onClick={() => removeFilterCondition(index)}
              />
            </Col>
          </Row>
        ))}

        {filters.conditions.length === 0 && (
          <div className={styles.empty}>
            <p>No filter conditions</p>
            <p className={styles.hint}>Click Add to create a filter</p>
          </div>
        )}
      </div>
    </Card>
  );
};

export default FilterPanel;

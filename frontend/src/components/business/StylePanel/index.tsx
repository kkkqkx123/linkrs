import React from 'react';
import { Card, Collapse, Select, Radio, Space, Button } from 'antd';
import { useGraphStore } from '@/stores/graph';
import styles from './index.module.less';

const { Panel } = Collapse;
const { Option } = Select;

const COLORS = [
  '#1890ff', '#52c41a', '#faad14', '#f5222d', '#722ed1',
  '#13c2c2', '#eb2f96', '#fa8c16', '#a0d911', '#2f54eb',
];

const StylePanel: React.FC = () => {
  const {
    graphData,
    nodeStyles,
    edgeStyles,
    setNodeStyle,
    setEdgeStyle,
    resetStyles,
  } = useGraphStore();

  if (!graphData) return null;

  // Get unique tags and edge types
  const tags = [...new Set(graphData.nodes.map((n) => n.tag))];
  const edgeTypes = [...new Set(graphData.edges.map((e) => e.type))];

  // Get all property keys for label selection
  const getNodeProperties = (tag: string): string[] => {
    const node = graphData.nodes.find((n) => n.tag === tag);
    if (!node) return ['id'];
    return ['id', ...Object.keys(node.properties)];
  };

  const getEdgeProperties = (type: string): string[] => {
    const edge = graphData.edges.find((e) => e.type === type);
    if (!edge) return ['type'];
    return ['type', ...Object.keys(edge.properties)];
  };

  return (
    <Card
      title="Style Settings"
      size="small"
      className={styles.panel}
      extra={
        <Button type="link" size="small" onClick={resetStyles}>
          Reset
        </Button>
      }
    >
      <Collapse defaultActiveKey={['nodes']} ghost>
        <Panel header="Nodes" key="nodes">
          {tags.map((tag) => (
            <div key={tag} className={styles.styleSection}>
              <div className={styles.styleHeader}>{tag}</div>
              <Space direction="vertical" size="small" style={{ width: '100%' }}>
                <div className={styles.colorPicker}>
                  <span>Color:</span>
                  <div className={styles.colors}>
                    {COLORS.map((color) => (
                      <button
                        key={color}
                        className={styles.colorBtn}
                        style={{
                          backgroundColor: color,
                          border: nodeStyles[tag]?.color === color ? '2px solid #333' : 'none',
                        }}
                        onClick={() => setNodeStyle(tag, { color })}
                      />
                    ))}
                  </div>
                </div>
                <div className={styles.sizePicker}>
                  <span>Size:</span>
                  <Radio.Group
                    value={nodeStyles[tag]?.size || 'medium'}
                    onChange={(e) => setNodeStyle(tag, { size: e.target.value })}
                    size="small"
                  >
                    <Radio.Button value="small">Small</Radio.Button>
                    <Radio.Button value="medium">Medium</Radio.Button>
                    <Radio.Button value="large">Large</Radio.Button>
                  </Radio.Group>
                </div>
                <div className={styles.labelPicker}>
                  <span>Label:</span>
                  <Select
                    value={nodeStyles[tag]?.labelProperty || 'id'}
                    onChange={(value) => setNodeStyle(tag, { labelProperty: value })}
                    size="small"
                    style={{ width: 120 }}
                  >
                    {getNodeProperties(tag).map((prop) => (
                      <Option key={prop} value={prop}>
                        {prop}
                      </Option>
                    ))}
                  </Select>
                </div>
              </Space>
            </div>
          ))}
        </Panel>

        {edgeTypes.length > 0 && (
          <Panel header="Edges" key="edges">
            {edgeTypes.map((type) => (
              <div key={type} className={styles.styleSection}>
                <div className={styles.styleHeader}>{type}</div>
                <Space direction="vertical" size="small" style={{ width: '100%' }}>
                  <div className={styles.colorPicker}>
                    <span>Color:</span>
                    <div className={styles.colors}>
                      {COLORS.map((color) => (
                        <button
                          key={color}
                          className={styles.colorBtn}
                          style={{
                            backgroundColor: color,
                            border: edgeStyles[type]?.color === color ? '2px solid #333' : 'none',
                          }}
                          onClick={() => setEdgeStyle(type, { color })}
                        />
                      ))}
                    </div>
                  </div>
                  <div className={styles.sizePicker}>
                    <span>Width:</span>
                    <Radio.Group
                      value={edgeStyles[type]?.width || 'medium'}
                      onChange={(e) => setEdgeStyle(type, { width: e.target.value })}
                      size="small"
                    >
                      <Radio.Button value="thin">Thin</Radio.Button>
                      <Radio.Button value="medium">Medium</Radio.Button>
                      <Radio.Button value="thick">Thick</Radio.Button>
                    </Radio.Group>
                  </div>
                  <div className={styles.labelPicker}>
                    <span>Label:</span>
                    <Select
                      value={edgeStyles[type]?.labelProperty || 'type'}
                      onChange={(value) => setEdgeStyle(type, { labelProperty: value })}
                      size="small"
                      style={{ width: 120 }}
                    >
                      {getEdgeProperties(type).map((prop) => (
                        <Option key={prop} value={prop}>
                          {prop}
                        </Option>
                      ))}
                    </Select>
                  </div>
                </Space>
              </div>
            ))}
          </Panel>
        )}
      </Collapse>
    </Card>
  );
};

export default StylePanel;

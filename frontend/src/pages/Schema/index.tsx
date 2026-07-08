import React from 'react';
import { Routes, Route, Navigate } from 'react-router-dom';
import SpaceList from './SpaceList';
import TagList from './TagList';
import EdgeList from './EdgeList';
import IndexList from './IndexList';
import SchemaVisualization from './SchemaVisualization';
import SpaceStats from './SpaceStats';

const Schema: React.FC = () => {
  return (
    <Routes>
      <Route path="/" element={<Navigate to="spaces" replace />} />
      <Route path="spaces" element={<SpaceList />} />
      <Route path="tags" element={<TagList />} />
      <Route path="edges" element={<EdgeList />} />
      <Route path="indexes" element={<IndexList />} />
      <Route path="visualization" element={<SchemaVisualization />} />
      <Route path="stats" element={<SpaceStats />} />
    </Routes>
  );
};

export default Schema;

import React, { useState, useEffect } from 'react';
import { Card, Typography, Spin } from 'antd';
import { CodeOutlined } from '@ant-design/icons';
import QueryEditor from './components/QueryEditor';
import OutputBox from './components/OutputBox';
import HistoryPanel from './components/HistoryPanel';
import FavoritePanel from './components/FavoritePanel';
import SaveFavoriteModal from './components/SaveFavoriteModal';
import { useConsoleStore, initEditorFromDraft } from '@/stores/console';
import styles from './index.module.less';

const { Title } = Typography;

const Console: React.FC = () => {
  // Panel visibility states
  const [historyOpen, setHistoryOpen] = useState(false);
  const [favoritesOpen, setFavoritesOpen] = useState(false);
  const [saveModalOpen, setSaveModalOpen] = useState(false);

  // Get console state
  const {
    currentResult,
    error,
    executionTime,
    activeView,
    isExecuting,
    setActiveView,
  } = useConsoleStore();

  // Initialize editor from draft on mount
  useEffect(() => {
    initEditorFromDraft();
  }, []);

  // Handlers
  const handleOpenHistory = () => setHistoryOpen(true);
  const handleCloseHistory = () => setHistoryOpen(false);
  
  const handleOpenFavorites = () => setFavoritesOpen(true);
  const handleCloseFavorites = () => setFavoritesOpen(false);
  
  const handleOpenSaveModal = () => setSaveModalOpen(true);
  const handleCloseSaveModal = () => setSaveModalOpen(false);

  return (
    <div className={styles.console}>
      {/* Page Header */}
      <Card className={styles.headerCard}>
        <div className={styles.header}>
          <Title level={4} className={styles.title}>
            <CodeOutlined />
            Query Console
          </Title>
        </div>
      </Card>

      {/* Main Content */}
      <div className={styles.content}>
        {/* Editor Section */}
        <div className={styles.editorSection}>
          <QueryEditor
            onOpenHistory={handleOpenHistory}
            onOpenFavorites={handleOpenFavorites}
            onSaveFavorite={handleOpenSaveModal}
          />
        </div>

        {/* Result Section */}
        <div className={styles.resultSection}>
          {isExecuting ? (
            <div className={styles.loading}>
              <Spin size="large" tip="Executing query..." />
            </div>
          ) : (
            <OutputBox
              result={currentResult}
              error={error}
              executionTime={executionTime}
              activeView={activeView}
              onViewChange={setActiveView}
            />
          )}
        </div>
      </div>

      {/* Side Panels */}
      <HistoryPanel
        open={historyOpen}
        onClose={handleCloseHistory}
      />

      <FavoritePanel
        open={favoritesOpen}
        onClose={handleCloseFavorites}
        onSaveNew={handleOpenSaveModal}
      />

      <SaveFavoriteModal
        open={saveModalOpen}
        onClose={handleCloseSaveModal}
      />
    </div>
  );
};

export default Console;

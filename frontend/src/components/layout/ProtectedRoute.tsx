import React, { useEffect, useState } from 'react';
import { Navigate, useLocation } from 'react-router-dom';
import { useConnectionStore } from '@/stores/connection';
import LoadingScreen from '@/components/common/LoadingScreen';

const ProtectedRoute: React.FC<{ children: React.ReactNode }> = ({ children }) => {
  const { isConnected, isVerified, checkHealth } = useConnectionStore();
  const location = useLocation();
  const [isChecking, setIsChecking] = useState(true);

  useEffect(() => {
    const verifyConnection = async () => {
      if (isConnected && !isVerified) {
        await checkHealth();
      }
      setIsChecking(false);
    };

    verifyConnection();
  }, [isConnected, isVerified, checkHealth]);

  if (isChecking) {
    return <LoadingScreen />;
  }

  if (!isVerified) {
    return <Navigate to="/login" state={{ from: location }} replace />;
  }

  return <>{children}</>;
};

export default ProtectedRoute;

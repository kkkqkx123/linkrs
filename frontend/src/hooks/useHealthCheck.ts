import { useEffect, useRef } from 'react';
import { useConnectionStore } from '@/stores/connection';
import { HEALTH_CHECK_INTERVAL } from '@/utils/constants';
import { message } from 'antd';
import { useNavigate } from 'react-router-dom';

export const useHealthCheck = (enabled: boolean = true) => {
  const { checkHealth, isConnected } = useConnectionStore();
  const navigate = useNavigate();
  const intervalRef = useRef<number | null>(null);
  const hasShownWarning = useRef(false);

  useEffect(() => {
    if (!enabled || !isConnected) {
      if (intervalRef.current) {
        clearInterval(intervalRef.current);
        intervalRef.current = null;
      }
      hasShownWarning.current = false;
      return;
    }

    const performHealthCheck = async () => {
      try {
        const isHealthy = await checkHealth();
        
        if (!isHealthy && !hasShownWarning.current) {
          hasShownWarning.current = true;
          message.warning('Connection lost. Please reconnect.');
          
          setTimeout(() => {
            navigate('/login');
          }, 2000);
        }
      } catch (error) {
        console.error('Health check error:', error);
      }
    };

    intervalRef.current = setInterval(performHealthCheck, HEALTH_CHECK_INTERVAL);

    return () => {
      if (intervalRef.current) {
        clearInterval(intervalRef.current);
        intervalRef.current = null;
      }
    };
  }, [enabled, isConnected, checkHealth, navigate]);
};

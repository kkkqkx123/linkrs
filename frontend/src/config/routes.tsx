import { lazy, Suspense } from 'react';
import { createBrowserRouter, Navigate, Outlet } from 'react-router-dom';
import MainLayout from '@/components/layout/MainLayout';
import ProtectedRoute from '@/components/layout/ProtectedRoute';
import LoadingFallback from '@/components/common/LoadingFallback';

// eslint-disable-next-line react-refresh/only-export-components
const Login = lazy(() => import('@/pages/Login'));
// eslint-disable-next-line react-refresh/only-export-components
const MainPage = lazy(() => import('@/pages/MainPage'));
// eslint-disable-next-line react-refresh/only-export-components
const Console = lazy(() => import('@/pages/Console'));
// eslint-disable-next-line react-refresh/only-export-components
const Schema = lazy(() => import('@/pages/Schema'));
// eslint-disable-next-line react-refresh/only-export-components
const Graph = lazy(() => import('@/pages/Graph'));
// eslint-disable-next-line react-refresh/only-export-components
const DataBrowser = lazy(() => import('@/pages/DataBrowser'));

const router = createBrowserRouter([
  {
    path: '/login',
    element: (
      <Suspense fallback={<LoadingFallback />}>
        <Login />
      </Suspense>
    ),
  },
  {
    path: '/',
    element: (
      <ProtectedRoute>
        <MainLayout>
          <Outlet />
        </MainLayout>
      </ProtectedRoute>
    ),
    children: [
      {
        index: true,
        element: (
          <Suspense fallback={<LoadingFallback />}>
            <MainPage />
          </Suspense>
        ),
      },
      {
        path: 'console',
        element: (
          <Suspense fallback={<LoadingFallback />}>
            <Console />
          </Suspense>
        ),
      },
      {
        path: 'schema',
        element: (
          <Suspense fallback={<LoadingFallback />}>
            <Schema />
          </Suspense>
        ),
      },
      {
        path: 'graph',
        element: (
          <Suspense fallback={<LoadingFallback />}>
            <Graph />
          </Suspense>
        ),
      },
      {
        path: 'data-browser',
        element: (
          <Suspense fallback={<LoadingFallback />}>
            <DataBrowser />
          </Suspense>
        ),
      },
    ],
  },
  {
    path: '*',
    element: <Navigate to="/" replace />,
  },
]);

export default router;

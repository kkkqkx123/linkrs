import axios, { type AxiosInstance, type AxiosRequestConfig } from 'axios';
import JSONBigint from 'json-bigint';

const JSONBigintInstance = JSONBigint({ storeAsString: true });

let serviceInstance: AxiosInstance | null = null;

let onUnauthorized: (() => void) | null = null;

export function setUnauthorizedHandler(handler: () => void) {
  onUnauthorized = handler;
}

const initService = (service?: AxiosInstance) => {
  if (service) {
    serviceInstance = service;
    return;
  }

  serviceInstance = axios.create({
    baseURL: import.meta.env.VITE_API_BASE_URL || 'http://localhost:9758',
    timeout: 30000,
    transformResponse: [
      (data) => {
        try {
          return JSONBigintInstance.parse(data);
        } catch {
          try {
            return JSON.parse(data);
          } catch {
            return data;
          }
        }
      },
    ],
  });

  serviceInstance.interceptors.request.use(
    (config) => {
      config.headers['Content-Type'] = 'application/json';
      const sessionId = localStorage.getItem('sessionId');
      if (sessionId) {
        config.headers['X-Session-ID'] = sessionId;
      }
      return config;
    },
    (error) => Promise.reject(error),
  );

  serviceInstance.interceptors.response.use(
    (response) => response.data,
    (error) => {
      if (error.response) {
        const { status, data } = error.response;
        if (data?.error?.message) {
          console.error(data.error.message);
        } else if (data?.message) {
          console.error(data.message);
        }

        if (status === 401) {
          localStorage.removeItem('sessionId');
          onUnauthorized?.();
        }
        return Promise.reject(error);
      } else if (!axios.isCancel(error)) {
        console.error('Network Error:', error.message);
      }
      return Promise.reject(error);
    },
  );
};

const sendRequest = async (
  type: string,
  api: string,
  params?: unknown,
  config?: AxiosRequestConfig,
): Promise<unknown> => {
  if (!serviceInstance) initService();

  let res;
  switch (type) {
    case 'get':
      res = await serviceInstance!.get(api, { params, ...config });
      break;
    case 'post':
      res = await serviceInstance!.post(api, params, config);
      break;
    case 'put':
      res = await serviceInstance!.put(api, params, config);
      break;
    case 'delete':
      res = await serviceInstance!.delete(api, { params, ...config });
      break;
    default:
      throw new Error(`Unsupported request type: ${type}`);
  }
  return res;
};

export const get = (api: string) => (params?: object, config: AxiosRequestConfig = {}) =>
  sendRequest('get', api, params, config);

export const post = (api: string) => (params?: object, config: AxiosRequestConfig = {}) =>
  sendRequest('post', api, params, config);

export const put = (api: string) => (params?: object, config: AxiosRequestConfig = {}) =>
  sendRequest('put', api, params, config);

export const _delete = (api: string) => (params?: object, config: AxiosRequestConfig = {}) =>
  sendRequest('delete', api, params, config);

export { initService };
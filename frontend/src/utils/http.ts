import axios, { type AxiosInstance, type AxiosRequestConfig, type AxiosResponse } from 'axios';
import { message } from 'antd';
import JSONBigint from 'json-bigint';

const JSONBigintInstance = JSONBigint({ storeAsString: true });

let serviceInstance: AxiosInstance | null = null;

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
    (error) => {
      return Promise.reject(error);
    }
  );

  serviceInstance.interceptors.response.use(
    (response: AxiosResponse) => {
      return response.data;
    },
    (error) => {
      if (error.response) {
        const { status, data } = error.response;
        const { hideErrMsg } = error.response.config || {};

        if (data?.error?.message) {
          if (!hideErrMsg) {
            message.error(data.error.message);
          }
        } else if (data?.message) {
          if (!hideErrMsg) {
            message.error(data.message);
          }
        } else {
          message.error(`Request Error: ${status} ${error.response.statusText}`);
        }

        if (status === 401) {
          localStorage.removeItem('sessionId');
          window.location.href = '/login';
        }

        return Promise.reject(error);
      } else if (!axios.isCancel(error)) {
        message.error(`Network Error: ${error.message}`);
      }

      return Promise.reject(error);
    }
  );
};

const sendRequest = async (
  type: string,
  api: string,
  params?: unknown,
  config?: AxiosRequestConfig
): Promise<unknown> => {
  if (!serviceInstance) {
    initService();
  }

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

const get =
  (api: string) =>
  (params?: object, config: AxiosRequestConfig = {}) =>
    sendRequest('get', api, params, config);

const post =
  (api: string) =>
  (params?: object, config: AxiosRequestConfig = {}) =>
    sendRequest('post', api, params, config);

const put =
  (api: string) =>
  (params?: object, config: AxiosRequestConfig = {}) =>
    sendRequest('put', api, params, config);

const _delete =
  (api: string) =>
  (params?: object, config: AxiosRequestConfig = {}) =>
    sendRequest('delete', api, params, config);

export { initService, get, post, put, _delete };

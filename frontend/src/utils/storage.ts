const STORAGE_PREFIX = 'graphdb_';

export const storage = {
  set: (key: string, value: unknown): void => {
    try {
      const serializedValue = JSON.stringify(value);
      localStorage.setItem(`${STORAGE_PREFIX}${key}`, serializedValue);
    } catch (error) {
      console.error('Error saving to localStorage:', error);
    }
  },

  get: <T>(key: string, defaultValue?: T): T | null => {
    try {
      const item = localStorage.getItem(`${STORAGE_PREFIX}${key}`);
      if (item === null) {
        return defaultValue ?? null;
      }
      return JSON.parse(item) as T;
    } catch (error) {
      console.error('Error reading from localStorage:', error);
      return defaultValue ?? null;
    }
  },

  remove: (key: string): void => {
    try {
      localStorage.removeItem(`${STORAGE_PREFIX}${key}`);
    } catch (error) {
      console.error('Error removing from localStorage:', error);
    }
  },

  clear: (): void => {
    try {
      const keys = Object.keys(localStorage);
      keys.forEach((key) => {
        if (key.startsWith(STORAGE_PREFIX)) {
          localStorage.removeItem(key);
        }
      });
    } catch (error) {
      console.error('Error clearing localStorage:', error);
    }
  },

  has: (key: string): boolean => {
    try {
      return localStorage.getItem(`${STORAGE_PREFIX}${key}`) !== null;
    } catch (error) {
      console.error('Error checking localStorage:', error);
      return false;
    }
  },
};

export const session = {
  set: (key: string, value: unknown): void => {
    try {
      const serializedValue = JSON.stringify(value);
      sessionStorage.setItem(`${STORAGE_PREFIX}${key}`, serializedValue);
    } catch (error) {
      console.error('Error saving to sessionStorage:', error);
    }
  },

  get: <T>(key: string, defaultValue?: T): T | null => {
    try {
      const item = sessionStorage.getItem(`${STORAGE_PREFIX}${key}`);
      if (item === null) {
        return defaultValue ?? null;
      }
      return JSON.parse(item) as T;
    } catch (error) {
      console.error('Error reading from sessionStorage:', error);
      return defaultValue ?? null;
    }
  },

  remove: (key: string): void => {
    try {
      sessionStorage.removeItem(`${STORAGE_PREFIX}${key}`);
    } catch (error) {
      console.error('Error removing from sessionStorage:', error);
    }
  },

  clear: (): void => {
    try {
      const keys = Object.keys(sessionStorage);
      keys.forEach((key) => {
        if (key.startsWith(STORAGE_PREFIX)) {
          sessionStorage.removeItem(key);
        }
      });
    } catch (error) {
      console.error('Error clearing sessionStorage:', error);
    }
  },
};

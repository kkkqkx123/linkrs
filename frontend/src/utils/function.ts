export const handleKeyword = (name: string) => {
  return `\`${handleEscape(name)}\``;
};

export const handleEscape = (name: string) => name.replace(/\\/gm, '\\\\').replace(/`/gm, '\\`');

export const handleVidStringName = (name: string, spaceVidType?: string) => {
  if (spaceVidType && spaceVidType === 'INT64') {
    return convertBigNumberToString(name);
  }
  return JSON.stringify(name);
};

export const convertBigNumberToString = (value: unknown) => {
  return typeof value === 'bigint' ? value.toString() : value;
};

export const removeNullCharacters = (data: string) => {
  // eslint-disable-next-line no-control-regex
  return data.replace(/\u0000+$/g, '');
};

export const safeParse = <T>(
  data: string,
  options?: { parser?: (data: string) => T },
): T | undefined => {
  const { parser } = options || {};
  try {
    return parser ? parser(data) : JSON.parse(data);
  } catch (err) {
    console.error('JSON.parse error', err);
    return undefined;
  }
};

export const getByteLength = (str: string) => {
  const utf8Encode = new TextEncoder();
  return utf8Encode.encode(str).length;
};

export const isValidIP = (ip: string) => {
  const reg = /^(\d{1,2}|1\d\d|2[0-4]\d|25[0-5])(\.(\d{1,2}|1\d\d|2[0-4]\d|25[0-5])){3}$/;
  return reg.test(ip);
};

export const isEmpty = (value: unknown) => {
  return !value && value !== 0;
};

export const debounce = <T extends (...args: unknown[]) => unknown>(
  func: T,
  wait: number
): ((...args: Parameters<T>) => void) => {
  let timeout: ReturnType<typeof setTimeout> | null = null;
  
  return function executedFunction(...args: Parameters<T>) {
    const later = () => {
      timeout = null;
      func(...args);
    };
    
    if (timeout) {
      clearTimeout(timeout);
    }
    timeout = setTimeout(later, wait);
  };
};

export const throttle = <T extends (...args: unknown[]) => unknown>(
  func: T,
  limit: number
): ((...args: Parameters<T>) => void) => {
  let inThrottle: boolean;
  
  return function executedFunction(...args: Parameters<T>) {
    if (!inThrottle) {
      func(...args);
      inThrottle = true;
      setTimeout(() => (inThrottle = false), limit);
    }
  };
};

export const formatDate = (date: Date | string, format: string = 'YYYY-MM-DD HH:mm:ss') => {
  const d = typeof date === 'string' ? new Date(date) : date;
  
  const year = d.getFullYear();
  const month = String(d.getMonth() + 1).padStart(2, '0');
  const day = String(d.getDate()).padStart(2, '0');
  const hours = String(d.getHours()).padStart(2, '0');
  const minutes = String(d.getMinutes()).padStart(2, '0');
  const seconds = String(d.getSeconds()).padStart(2, '0');
  
  return format
    .replace('YYYY', String(year))
    .replace('MM', month)
    .replace('DD', day)
    .replace('HH', hours)
    .replace('mm', minutes)
    .replace('ss', seconds);
};

export const copyToClipboard = async (text: string): Promise<boolean> => {
  try {
    if (navigator.clipboard && navigator.clipboard.writeText) {
      await navigator.clipboard.writeText(text);
      return true;
    }
    
    const textArea = document.createElement('textarea');
    textArea.value = text;
    textArea.style.position = 'fixed';
    textArea.style.left = '-999999px';
    textArea.style.top = '-999999px';
    document.body.appendChild(textArea);
    textArea.focus();
    textArea.select();
    
    try {
      document.execCommand('copy');
      return true;
    } catch (err) {
      console.error('Failed to copy:', err);
      return false;
    } finally {
      document.body.removeChild(textArea);
    }
  } catch (err) {
    console.error('Failed to copy:', err);
    return false;
  }
};

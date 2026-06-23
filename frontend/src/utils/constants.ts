export const DATA_TYPES = {
  BOOL: 'bool',
  INT8: 'int8',
  INT16: 'int16',
  INT32: 'int32',
  INT64: 'int64',
  FLOAT: 'float',
  DOUBLE: 'double',
  STRING: 'string',
  DATE: 'date',
  TIME: 'time',
  DATETIME: 'datetime',
  FIXED_STRING: 'fixed_string',
  TIMESTAMP: 'timestamp',
  GEOGRAPHY: 'geography',
  GEOGRAPHY_POINT: 'geography(point)',
  GEOGRAPHY_LINESTRING: 'geography(linestring)',
  GEOGRAPHY_POLYGON: 'geography(polygon)',
  DURATION: 'duration',
  JSON: 'json',
  UNKNOWN: 'unknown',
} as const;

export const DATA_TYPE_LABELS = {
  [DATA_TYPES.BOOL]: 'Boolean',
  [DATA_TYPES.INT8]: 'Int8',
  [DATA_TYPES.INT16]: 'Int16',
  [DATA_TYPES.INT32]: 'Int32',
  [DATA_TYPES.INT64]: 'Int64',
  [DATA_TYPES.FLOAT]: 'Float',
  [DATA_TYPES.DOUBLE]: 'Double',
  [DATA_TYPES.STRING]: 'String',
  [DATA_TYPES.DATE]: 'Date',
  [DATA_TYPES.TIME]: 'Time',
  [DATA_TYPES.DATETIME]: 'DateTime',
  [DATA_TYPES.FIXED_STRING]: 'Fixed String',
  [DATA_TYPES.TIMESTAMP]: 'Timestamp',
  [DATA_TYPES.GEOGRAPHY]: 'Geography',
  [DATA_TYPES.GEOGRAPHY_POINT]: 'Geography Point',
  [DATA_TYPES.GEOGRAPHY_LINESTRING]: 'Geography LineString',
  [DATA_TYPES.GEOGRAPHY_POLYGON]: 'Geography Polygon',
  [DATA_TYPES.DURATION]: 'Duration',
  [DATA_TYPES.JSON]: 'JSON',
  [DATA_TYPES.UNKNOWN]: 'Unknown',
} as const;

export const OPERATORS = {
  EQ: '==',
  NEQ: '!=',
  LT: '<',
  LTE: '<=',
  GT: '>',
  GTE: '>=',
  IN: 'IN',
  NOT_IN: 'NOT IN',
  CONTAINS: 'CONTAINS',
  STARTS_WITH: 'STARTS WITH',
  ENDS_WITH: 'ENDS WITH',
  MATCHES: '=~',
  AND: 'AND',
  OR: 'OR',
  NOT: 'NOT',
} as const;

export const VID_TYPES = {
  INT64: 'INT64',
  FIXED_STRING: 'FIXED_STRING',
} as const;

export const DEFAULT_VALUES = {
  USERNAME: 'root',
  PASSWORD: '',
} as const;

export const STORAGE_KEYS = {
  CONNECTION: 'connection',
  SESSION_ID: 'sessionId',
  REMEMBER_ME: 'rememberMe',
} as const;

export const ROUTES = {
  LOGIN: '/login',
  HOME: '/',
  CONSOLE: '/console',
  SCHEMA: '/schema',
  GRAPH: '/graph',
  DATA_BROWSER: '/data-browser',
} as const;

export const HTTP_STATUS = {
  OK: 200,
  CREATED: 201,
  NO_CONTENT: 204,
  BAD_REQUEST: 400,
  UNAUTHORIZED: 401,
  FORBIDDEN: 403,
  NOT_FOUND: 404,
  INTERNAL_SERVER_ERROR: 500,
} as const;

export const HEALTH_CHECK_INTERVAL = 5000;

export const MAX_RETRY_ATTEMPTS = 3;

export const REQUEST_TIMEOUT = 30000;

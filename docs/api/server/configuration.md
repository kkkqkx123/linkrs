## 9. Configuration APIs

### 9.1 Get Configuration

**Endpoint**: `GET /v1/config`

**Description**: Get current server configuration.

**Response**:

```json
{
  "database": {
    "host": "127.0.0.1",
    "port": 3699,
    "storage_path": "./data",
    "max_connections": 100
  },
  "transaction": {
    "default_timeout": 30,
    "max_concurrent_transactions": 100
  },
  "log": {
    "level": "info",
    "dir": "./logs",
    "file": "linkrs.log",
    "max_file_size": 104857600,
    "max_files": 5
  },
  "auth": {
    "enable_authorize": true,
    "failed_login_attempts": 3,
    "session_idle_timeout_secs": 3600,
    "force_change_default_password": true,
    "default_username": "root"
  },
  "bootstrap": {
    "auto_create_default_space": true,
    "default_space_name": "default",
    "single_user_mode": false
  },
  "optimizer": {
    "max_iteration_rounds": 100,
    "max_exploration_rounds": 10,
    "enable_cost_model": true,
    "enable_multi_plan": true,
    "enable_property_pruning": true,
    "enable_adaptive_iteration": true,
    "stable_threshold": 5,
    "min_iteration_rounds": 10
  },
  "monitoring": {
    "enabled": true,
    "memory_cache_size": 1000,
    "slow_query_threshold_ms": 1000
  }
}
```

**Status Codes**:

- `200 OK`: Configuration retrieved

---

### 9.2 Update Configuration

**Endpoint**: `PUT /v1/config`

**Description**: Update server configuration (hot update where supported).

**Request Body**:

```json
{
  "database": {
    "max_connections": 200
  }
}
```

**Response**:

```json
{
  "updated": ["database.max_connections"],
  "requires_restart": [],
  "message": "Configuration update received, some changes may require restart to take effect"
}
```

**Status Codes**:

- `200 OK`: Configuration updated

---

### 9.3 Get Configuration Key

**Endpoint**: `GET /v1/config/{section}/{key}`

**Description**: Get a specific configuration value.

**Path Parameters**:

- `section` (String): Configuration section
- `key` (String): Configuration key

**Response**:

```json
{
  "section": "database",
  "key": "port",
  "value": 3699
}
```

**Status Codes**:

- `200 OK`: Value retrieved

---

### 9.4 Update Configuration Key

**Endpoint**: `PUT /v1/config/{section}/{key}`

**Description**: Update a specific configuration value.

**Path Parameters**:

- `section` (String): Configuration section
- `key` (String): Configuration key

**Request Body**:

```json
{
  "value": 3700
}
```

**Response**:

```json
{
  "section": "database",
  "key": "port",
  "value": 3700,
  "requires_restart": true,
  "message": "Configuration item updated, but restart required to take effect"
}
```

**Status Codes**:

- `200 OK`: Value updated

---

### 9.5 Reset Configuration Key

**Endpoint**: `DELETE /v1/config/{section}/{key}`

**Description**: Reset a configuration value to default.

**Path Parameters**:

- `section` (String): Configuration section
- `key` (String): Configuration key

**Response**:

```json
{
  "section": "database",
  "key": "port",
  "value": 3699,
  "message": "Configuration reset to default value"
}
```

**Status Codes**:

- `200 OK`: Value reset

---


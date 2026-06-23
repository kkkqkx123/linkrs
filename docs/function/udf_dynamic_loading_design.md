# UDF 动态加载架构设计方案

## 1. 概述

### 1.1 背景
当前项目的函数系统仅支持静态注册（代码级注册），缺乏像 Nebula-Graph 那样的动态 UDF（User Defined Function）加载能力。为了实现运行时扩展功能，需要设计一套 UDF 动态加载机制。

### 1.2 目标
- 支持从动态库（DLL/SO）加载用户自定义函数
- 保持与现有函数注册系统的兼容性
- 提供安全的 UDF 执行环境
- 支持 UDF 的热加载和卸载

### 1.3 参考实现
- Nebula-Graph 的 `FunctionUdfManager` + `GraphFunction` 接口
- Rust 的 `libloading` crate 用于动态库加载

---

## 2. 架构设计

### 2.1 整体架构

```
┌─────────────────────────────────────────────────────────────┐
│                    FunctionRegistry                          │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────────┐   │
│  │   Builtin    │  │   Custom     │  │  Dynamic (UDF)   │   │
│  │  Functions   │  │  Functions   │  │   Functions      │   │
│  └──────────────┘  └──────────────┘  └──────────────────┘   │
│                                    │                        │
│                                    ▼                        │
│                         ┌──────────────────┐               │
│                         │  UdfManager      │               │
│                         │  (动态库管理)     │               │
│                         └──────────────────┘               │
└─────────────────────────────────────────────────────────────┘
```

### 2.2 核心组件

| 组件 | 职责 | 文件路径 |
|------|------|----------|
| `UdfManager` | UDF 管理器，负责动态库的加载、卸载和生命周期管理 | `src/expression/functions/udf/manager.rs` |
| `UdfFunction` | UDF 函数包装器，实现 ExpressionFunction trait | `src/expression/functions/udf/function.rs` |
| `UdfPlugin` | UDF 插件 trait，定义 UDF 接口规范 | `src/expression/functions/udf/plugin.rs` |
| `UdfRegistry` | UDF 注册表，管理已加载的 UDF | `src/expression/functions/udf/registry.rs` |

---

## 3. 详细设计

### 3.1 UDF 接口定义

```rust
// src/expression/functions/udf/plugin.rs

use crate::core::Value;
use crate::core::error::ExpressionError;

/// UDF 插件接口
/// 
/// 用户实现的 UDF 需要实现此 trait
/// 通过动态库导出供数据库加载
pub trait UdfPlugin: Send + Sync {
    /// 获取函数名称
    fn name(&self) -> &str;
    
    /// 获取函数描述
    fn description(&self) -> &str;
    
    /// 获取输入参数类型列表
    /// 支持多签名重载，每个签名是一个参数类型列表
    fn input_types(&self) -> Vec<Vec<ValueType>>;
    
    /// 获取返回值类型
    fn return_type(&self) -> ValueType;
    
    /// 获取最小参数数量
    fn min_arity(&self) -> usize;
    
    /// 获取最大参数数量
    fn max_arity(&self) -> usize;
    
    /// 是否为纯函数（相同输入总是产生相同输出）
    fn is_pure(&self) -> bool;
    
    /// 执行函数
    fn execute(&self, args: &[Value]) -> Result<Value, ExpressionError>;
}

/// UDF 插件工厂函数类型
pub type UdfCreateFn = unsafe fn() -> *mut dyn UdfPlugin;

/// UDF 插件销毁函数类型
pub type UdfDestroyFn = unsafe fn(*mut dyn UdfPlugin);

/// 导出符号名称
pub const UDF_CREATE_SYMBOL: &str = "udf_create";
pub const UDF_DESTROY_SYMBOL: &str = "udf_destroy";
```

### 3.2 UDF 管理器

```rust
// src/expression/functions/udf/manager.rs

use libloading::{Library, Symbol};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::Duration;

/// UDF 库元数据
struct UdfLibrary {
    /// 动态库句柄
    library: Library,
    /// 库文件路径
    path: PathBuf,
    /// 库中导出的函数名称列表
    functions: Vec<String>,
    /// 加载时间
    loaded_at: std::time::Instant,
}

/// UDF 管理器
pub struct UdfManager {
    /// 已加载的动态库
    libraries: RwLock<HashMap<String, UdfLibrary>>,
    /// UDF 注册表
    registry: Arc<RwLock<UdfRegistry>>,
    /// UDF 搜索路径
    udf_paths: Vec<PathBuf>,
    /// 是否启用 UDF
    enabled: bool,
    /// 自动重载间隔（秒），0 表示禁用
    auto_reload_interval: Duration,
}

impl UdfManager {
    /// 创建新的 UDF 管理器
    pub fn new(config: UdfConfig) -> Result<Self, UdfError> {
        let manager = Self {
            libraries: RwLock::new(HashMap::new()),
            registry: Arc::new(RwLock::new(UdfRegistry::new())),
            udf_paths: config.paths,
            enabled: config.enabled,
            auto_reload_interval: config.auto_reload_interval,
        };
        
        if manager.enabled {
            manager.init_and_load_all()?;
            manager.start_auto_reload_thread();
        }
        
        Ok(manager)
    }
    
    /// 从指定路径加载 UDF 库
    pub fn load_library(&self, path: &Path) -> Result<Vec<String>, UdfError> {
        // 1. 加载动态库
        let library = unsafe { Library::new(path) }
            .map_err(|e| UdfError::LoadFailed(format!("无法加载库: {}", e)))?;
        
        // 2. 获取创建函数
        let create_fn: Symbol<UdfCreateFn> = unsafe {
            library.get(UDF_CREATE_SYMBOL.as_bytes())
        }.map_err(|e| UdfError::SymbolNotFound(format!("找不到符号 {}: {}", UDF_CREATE_SYMBOL, e)))?;
        
        // 3. 获取销毁函数
        let destroy_fn: Symbol<UdfDestroyFn> = unsafe {
            library.get(UDF_DESTROY_SYMBOL.as_bytes())
        }.map_err(|e| UdfError::SymbolNotFound(format!("找不到符号 {}: {}", UDF_DESTROY_SYMBOL, e)))?;
        
        // 4. 创建 UDF 实例获取元数据
        let udf_ptr = unsafe { create_fn() };
        if udf_ptr.is_null() {
            return Err(UdfError::CreateFailed("UDF 创建失败".to_string()));
        }
        
        let udf = unsafe { &*udf_ptr };
        let function_name = udf.name().to_string();
        let metadata = UdfMetadata {
            name: function_name.clone(),
            description: udf.description().to_string(),
            input_types: udf.input_types(),
            return_type: udf.return_type(),
            min_arity: udf.min_arity(),
            max_arity: udf.max_arity(),
            is_pure: udf.is_pure(),
        };
        
        // 5. 销毁临时实例
        unsafe { destroy_fn(udf_ptr) };
        
        // 6. 注册到 UDF 注册表
        let udf_function = UdfFunction::new(
            metadata,
            path.to_path_buf(),
        );
        
        self.registry.write().unwrap().register(udf_function);
        
        // 7. 保存库信息
        let lib_name = path.file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "unknown".to_string());
        
        let udf_lib = UdfLibrary {
            library,
            path: path.to_path_buf(),
            functions: vec![function_name.clone()],
            loaded_at: std::time::Instant::now(),
        };
        
        self.libraries.write().unwrap().insert(lib_name, udf_lib);
        
        Ok(vec![function_name])
    }
    
    /// 卸载 UDF 库
    pub fn unload_library(&self, lib_name: &str) -> Result<(), UdfError> {
        let mut libraries = self.libraries.write().unwrap();
        
        if let Some(lib) = libraries.remove(lib_name) {
            // 从注册表中移除该库的所有函数
            let mut registry = self.registry.write().unwrap();
            for func_name in &lib.functions {
                registry.unregister(func_name);
            }
            // library 会在 drop 时自动卸载
        }
        
        Ok(())
    }
    
    /// 获取 UDF 注册表
    pub fn registry(&self) -> Arc<RwLock<UdfRegistry>> {
        self.registry.clone()
    }
    
    /// 初始化并加载所有 UDF
    fn init_and_load_all(&self) -> Result<(), UdfError> {
        for path in &self.udf_paths {
            if path.exists() && path.is_dir() {
                self.load_from_directory(path)?;
            }
        }
        Ok(())
    }
    
    /// 从目录加载所有 UDF
    fn load_from_directory(&self, dir: &Path) -> Result<(), UdfError> {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            
            // 根据平台检查文件扩展名
            #[cfg(target_os = "windows")]
            let is_lib = path.extension().map(|e| e == "dll").unwrap_or(false);
            #[cfg(target_os = "linux")]
            let is_lib = path.extension().map(|e| e == "so").unwrap_or(false);
            #[cfg(target_os = "macos")]
            let is_lib = path.extension().map(|e| e == "dylib").unwrap_or(false);
            
            if is_lib {
                if let Err(e) = self.load_library(&path) {
                    log::error!("加载 UDF 库失败 {:?}: {}", path, e);
                }
            }
        }
        Ok(())
    }
    
    /// 启动自动重载线程
    fn start_auto_reload_thread(&self) {
        if self.auto_reload_interval.as_secs() == 0 {
            return;
        }
        
        let interval = self.auto_reload_interval;
        // 启动后台线程定期检查并重新加载
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(interval);
                // 检查文件修改时间并重新加载
                // ...
            }
        });
    }
}

/// UDF 配置
pub struct UdfConfig {
    pub enabled: bool,
    pub paths: Vec<PathBuf>,
    pub auto_reload_interval: Duration,
}

impl Default for UdfConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            paths: vec![PathBuf::from("lib/udf")],
            auto_reload_interval: Duration::from_secs(300), // 5分钟
        }
    }
}
```

### 3.3 UDF 函数包装器

```rust
// src/expression/functions/udf/function.rs

use std::path::PathBuf;
use libloading::{Library, Symbol};

/// UDF 函数元数据
#[derive(Debug, Clone)]
pub struct UdfMetadata {
    pub name: String,
    pub description: String,
    pub input_types: Vec<Vec<ValueType>>,
    pub return_type: ValueType,
    pub min_arity: usize,
    pub max_arity: usize,
    pub is_pure: bool,
}

/// UDF 函数实现
pub struct UdfFunction {
    metadata: UdfMetadata,
    library_path: PathBuf,
}

impl UdfFunction {
    pub fn new(metadata: UdfMetadata, library_path: PathBuf) -> Self {
        Self {
            metadata,
            library_path,
        }
    }
    
    /// 动态执行 UDF
    fn execute_dynamic(&self, args: &[Value]) -> Result<Value, ExpressionError> {
        // 1. 加载库（每次执行时加载，执行后卸载）
        // 或者使用库缓存机制
        let library = unsafe { Library::new(&self.library_path) }
            .map_err(|e| ExpressionError::internal_error(format!("无法加载 UDF 库: {}", e)))?;
        
        // 2. 获取创建函数
        let create_fn: Symbol<UdfCreateFn> = unsafe {
            library.get(UDF_CREATE_SYMBOL.as_bytes())
        }.map_err(|e| ExpressionError::internal_error(format!("UDF 符号错误: {}", e)))?;
        
        let destroy_fn: Symbol<UdfDestroyFn> = unsafe {
            library.get(UDF_DESTROY_SYMBOL.as_bytes())
        }.map_err(|e| ExpressionError::internal_error(format!("UDF 符号错误: {}", e)))?;
        
        // 3. 创建实例并执行
        let udf_ptr = unsafe { create_fn() };
        if udf_ptr.is_null() {
            return Err(ExpressionError::internal_error("UDF 创建失败".to_string()));
        }
        
        let udf = unsafe { &*udf_ptr };
        let result = udf.execute(args);
        
        // 4. 销毁实例
        unsafe { destroy_fn(udf_ptr) };
        
        result
    }
}

impl ExpressionFunction for UdfFunction {
    fn name(&self) -> &str {
        &self.metadata.name
    }
    
    fn arity(&self) -> usize {
        self.metadata.min_arity
    }
    
    fn is_variadic(&self) -> bool {
        self.metadata.min_arity != self.metadata.max_arity
    }
    
    fn execute(&self, args: &[Value]) -> Result<Value, ExpressionError> {
        // 参数数量检查
        if args.len() < self.metadata.min_arity || args.len() > self.metadata.max_arity {
            return Err(ExpressionError::arity_error(
                &self.metadata.name,
                self.metadata.min_arity,
                self.metadata.max_arity,
                args.len(),
            ));
        }
        
        self.execute_dynamic(args)
    }
    
    fn description(&self) -> &str {
        &self.metadata.description
    }
}
```

### 3.4 UDF 注册表

```rust
// src/expression/functions/udf/registry.rs

use std::collections::HashMap;

/// UDF 注册表
pub struct UdfRegistry {
    functions: HashMap<String, UdfFunction>,
}

impl UdfRegistry {
    pub fn new() -> Self {
        Self {
            functions: HashMap::new(),
        }
    }
    
    /// 注册 UDF
    pub fn register(&mut self, function: UdfFunction) {
        let name = function.name().to_string();
        self.functions.insert(name, function);
    }
    
    /// 注销 UDF
    pub fn unregister(&mut self, name: &str) {
        self.functions.remove(name);
    }
    
    /// 获取 UDF
    pub fn get(&self, name: &str) -> Option<&UdfFunction> {
        self.functions.get(name)
    }
    
    /// 检查是否存在
    pub fn contains(&self, name: &str) -> bool {
        self.functions.contains_key(name)
    }
    
    /// 获取所有 UDF 名称
    pub fn function_names(&self) -> Vec<&str> {
        self.functions.keys().map(|s| s.as_str()).collect()
    }
}
```

---

## 4. 与现有系统集成

### 4.1 FunctionRegistry 集成

修改 `FunctionRegistry` 以支持 UDF：

```rust
// src/expression/functions/registry.rs

pub struct FunctionRegistry {
    functions: HashMap<String, Vec<RegisteredFunction>>,
    builtin_functions: HashMap<String, BuiltinFunction>,
    custom_functions: HashMap<String, CustomFunction>,
    // 新增：UDF 管理器
    udf_manager: Option<Arc<UdfManager>>,
}

impl FunctionRegistry {
    pub fn new() -> Self {
        let mut registry = Self {
            functions: HashMap::new(),
            builtin_functions: HashMap::new(),
            custom_functions: HashMap::new(),
            udf_manager: None,
        };
        registry.register_all_builtin_functions();
        registry
    }
    
    /// 启用 UDF 支持
    pub fn enable_udf(&mut self, config: UdfConfig) -> Result<(), UdfError> {
        let manager = UdfManager::new(config)?;
        self.udf_manager = Some(Arc::new(manager));
        Ok(())
    }
    
    /// 执行函数（优先查找内置，然后是 UDF）
    pub fn execute(&self, name: &str, args: &[Value]) -> Result<Value, ExpressionError> {
        // 1. 先查找内置函数
        if let Some(funcs) = self.functions.get(name) {
            // ... 原有逻辑
        }
        
        // 2. 查找 UDF
        if let Some(ref manager) = self.udf_manager {
            let registry = manager.registry().read().unwrap();
            if let Some(udf) = registry.get(name) {
                return udf.execute(args);
            }
        }
        
        Err(ExpressionError::undefined_function(name))
    }
}
```

### 4.2 配置集成

在配置文件中添加 UDF 配置：

```toml
# config.toml
[udf]
enabled = true
paths = ["lib/udf", "/usr/local/graphdb/udf"]
auto_reload_interval = 300  # 秒
```

---

## 5. UDF 开发示例

### 5.1 Rust UDF 示例

```rust
// my_udf.rs - 用户自定义函数库

use graphdb::udf::{UdfPlugin, Value, ValueType, ExpressionError};

pub struct MyCustomFunction;

impl UdfPlugin for MyCustomFunction {
    fn name(&self) -> &str {
        "my_custom_func"
    }
    
    fn description(&self) -> &str {
        "我的自定义函数：计算两个数的平方和"
    }
    
    fn input_types(&self) -> Vec<Vec<ValueType>> {
        vec![
            vec![ValueType::Int, ValueType::Int],
            vec![ValueType::Float, ValueType::Float],
        ]
    }
    
    fn return_type(&self) -> ValueType {
        ValueType::Float
    }
    
    fn min_arity(&self) -> usize {
        2
    }
    
    fn max_arity(&self) -> usize {
        2
    }
    
    fn is_pure(&self) -> bool {
        true
    }
    
    fn execute(&self, args: &[Value]) -> Result<Value, ExpressionError> {
        match (&args[0], &args[1]) {
            (Value::Int(a), Value::Int(b)) => {
                let result = (*a as f64).powi(2) + (*b as f64).powi(2);
                Ok(Value::Float(result))
            }
            (Value::Float(a), Value::Float(b)) => {
                let result = a.powi(2) + b.powi(2);
                Ok(Value::Float(result))
            }
            _ => Err(ExpressionError::type_error("参数类型不匹配")),
        }
    }
}

// 导出函数
#[no_mangle]
pub unsafe extern "C" fn udf_create() -> *mut dyn UdfPlugin {
    Box::into_raw(Box::new(MyCustomFunction))
}

#[no_mangle]
pub unsafe extern "C" fn udf_destroy(ptr: *mut dyn UdfPlugin) {
    if !ptr.is_null() {
        let _ = Box::from_raw(ptr);
    }
}
```

### 5.2 编译 UDF

```bash
# 编译为动态库
cargo build --release --crate-type cdylib

# 复制到 UDF 目录
cp target/release/libmy_udf.so lib/udf/
```

---

## 6. 安全考虑

### 6.1 安全风险

| 风险 | 描述 | 缓解措施 |
|------|------|----------|
| 代码注入 | 恶意 UDF 可能执行任意代码 | 1. 沙箱执行<br>2. 代码签名验证<br>3. 权限控制 |
| 内存泄漏 | UDF 可能分配内存不释放 | 1. 使用 RAII 模式<br>2. 资源使用限制 |
| 拒绝服务 | UDF 可能无限循环或占用大量资源 | 1. 执行超时<br>2. CPU/内存限制 |
| 类型混淆 | UDF 可能返回错误类型 | 1. 运行时类型检查<br>2. 返回值验证 |

### 6.2 安全建议

1. **代码签名**：要求 UDF 库必须经过数字签名才能加载
2. **沙箱执行**：在独立的进程中执行 UDF，限制其访问权限
3. **资源限制**：设置 UDF 执行的最大 CPU 时间和内存使用量
4. **审计日志**：记录所有 UDF 的加载和执行操作

---

## 7. 性能优化

### 7.1 优化策略

| 策略 | 描述 |
|------|------|
| 库缓存 | 避免重复加载相同的动态库 |
| 实例池 | 预创建 UDF 实例，减少创建开销 |
| 纯函数缓存 | 对纯函数的相同输入结果进行缓存 |
| 异步加载 | 后台线程加载 UDF，不阻塞主线程 |

### 7.2 性能对比

| 场景 | 静态函数 | UDF（无优化） | UDF（有优化） |
|------|----------|---------------|---------------|
| 首次调用 | 快 | 慢（加载库） | 慢（加载库） |
| 后续调用 | 快 | 慢（创建实例） | 快（实例池） |
| 纯函数重复调用 | 快 | 慢 | 快（结果缓存） |

---

## 8. 实现计划

### Phase 1: 基础框架
- [ ] 定义 `UdfPlugin` trait
- [ ] 实现 `UdfRegistry`
- [ ] 实现基础的 `UdfFunction`

### Phase 2: 动态加载
- [ ] 集成 `libloading` crate
- [ ] 实现 `UdfManager`
- [ ] 支持 Windows/Linux/Mac 平台

### Phase 3: 系统集成
- [ ] 修改 `FunctionRegistry` 支持 UDF
- [ ] 配置文件集成
- [ ] 错误处理和日志

### Phase 4: 优化和安全
- [ ] 实例池实现
- [ ] 纯函数缓存
- [ ] 基础安全限制

### Phase 5: 高级功能
- [ ] 热加载支持
- [ ] UDF 版本管理
- [ ] 代码签名验证

---

## 9. 与 Nebula-Graph 对比

| 特性 | Nebula (C++) | 本方案 (Rust) |
|------|--------------|---------------|
| 动态加载 | dlopen/dlsym | libloading |
| 接口类型 | 抽象基类 | Trait |
| 内存安全 | 手动管理 | Rust 所有权系统 |
| 并发安全 | 互斥锁 | RwLock + Arc |
| 跨平台 | 需要条件编译 | libloading 封装 |
| 性能 | 虚函数调用开销 | trait 对象开销 |
| 错误处理 | 返回码/异常 | Result 类型 |

---

## 10. 参考文档

- [libloading crate](https://docs.rs/libloading/)
- [Nebula Graph UDF 设计](https://github.com/vesoft-inc/nebula/blob/master/src/common/function/FunctionUdfManager.cpp)
- [Rust FFI 指南](https://doc.rust-lang.org/nomicon/ffi.html)

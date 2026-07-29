# 本地配置凭据保护实施计划

> **执行要求：** 使用 `superpowers:executing-plans` 在当前会话逐项执行；步骤使用复选框跟踪。

**目标：** 用系统安全存储中的随机主密钥替代机器标识派生密钥，并修复启动、迁移和配置异常时的运行逻辑。

**架构：** 新建 `session::config_key` 隔离系统凭据库访问，`ConfigStore` 持有进程内主密钥并读写版本 2 加密信封。版本 1 仅用于一次性迁移；主窗口只加载一次配置，失败时进入不可持久化的内存模式并显示通知。

**技术栈：** Rust 2024、`keyring 3.6`、XChaCha20-Poly1305、现有 Argon2 版本 1 兼容代码、GPUI Component Notification。

## 全局约束

- 不生成包含明文主密钥的 `.gen` 或其他本地密钥文件。
- 系统安全存储不可用时不得退回机器标识或固定常量。
- 版本 2 配置缺少主密钥时不得创建新密钥并覆盖原文件。
- 日志不得包含主密钥、配置明文、SSH 凭据或代理凭据。
- 不修改 `.github`、Release 工作流或发布脚本。
- 所有改动保留在工作区，不暂存、不提交。

---

### 任务 1：系统主密钥模块

**文件：**

- 新建：`src/session/config_key.rs`
- 修改：`src/session/mod.rs`
- 修改：`Cargo.toml`
- 修改：`Cargo.lock`

**接口：**

- `MasterKey::random() -> MasterKey`
- `MasterKey::from_secret(Vec<u8>) -> Result<MasterKey>`
- `MasterKey::as_bytes(&self) -> &[u8; 32]`
- `ConfigKeyStore::load(&self) -> Result<Option<MasterKey>>`
- `ConfigKeyStore::store(&self, key: &MasterKey) -> Result<()>`
- `load_existing_key(store: &dyn ConfigKeyStore) -> Result<MasterKey>`
- `load_or_create_key(store: &dyn ConfigKeyStore) -> Result<MasterKey>`

- [x] **步骤 1：编写失败测试**

使用内存假存储覆盖：首次创建 32 字节密钥、复用已有密钥、拒绝长度错误、读取错误不创建新密钥、写入错误直接返回。

- [x] **步骤 2：运行测试并确认因模块或接口缺失而失败**

运行：`cargo test --quiet session::config_key::tests`

- [x] **步骤 3：实现最小主密钥模块**

使用服务名 `dev.jshell.config`、账户名 `local-config-master-key`。平台实现启用 `apple-native`、`windows-native`、`sync-secret-service`、`crypto-rust` 和 `vendored`；成功读取后在进程内缓存，错误不缓存。

- [x] **步骤 4：运行模块测试和全目标编译**

运行：

```text
cargo test --quiet session::config_key::tests
cargo check --all-targets
```

---

### 任务 2：版本 2 加密信封

**文件：**

- 修改：`src/session/config.rs`

**接口：**

- `config_format_version(raw: &[u8]) -> Result<u32>`
- `encrypt_config_v2(config: &ConfigFile, key: &MasterKey) -> Result<Vec<u8>>`
- `decrypt_config_v2(raw: &[u8], key: &MasterKey) -> Result<ConfigFile>`
- `decrypt_config_v1(raw: &[u8], hardware_id: &str) -> Result<ConfigFile>`

- [x] **步骤 1：编写版本 2 失败测试**

覆盖版本字段、明文不可见、正确密钥往返、错误密钥失败、版本 2 不包含 Argon2 salt、版本 1 仍可由兼容函数读取。

- [x] **步骤 2：运行测试并确认新接口缺失**

运行：`cargo test --quiet config_encryption`

- [x] **步骤 3：实现版本分离**

版本 2 使用随机 nonce 和 `MasterKey` 直接加密；版本 1 保留现有 Argon2 逻辑并改名为迁移专用函数。错误信息区分不支持版本、信封损坏和认证失败。

- [x] **步骤 4：运行加密专项测试**

运行：`cargo test --quiet config_encryption`

---

### 任务 3：配置加载和一次性迁移

**文件：**

- 修改：`src/session/config.rs`

**接口：**

- `ConfigStore` 新增私有 `master_key: MasterKey`
- `ConfigStore::load_with_key_store(...) -> Result<ConfigStore>` 供生产和测试共享
- `ConfigStore::is_persistent(&self) -> bool`

- [x] **步骤 1：编写加载流程失败测试**

使用临时路径和内存密钥存储覆盖：无文件首次创建、版本 2 复用密钥、版本 2 缺密钥保持原字节不变、版本 1 自动迁移、迁移后使用错误机器标识仍可读取、损坏文件不覆盖。

- [x] **步骤 2：运行测试并确认当前逻辑不满足版本 2 行为**

运行：`cargo test --quiet config_store_encryption`

- [x] **步骤 3：实现格式分流和迁移**

加载前先识别信封版本。版本 2 只允许读取已有系统密钥；版本 1 成功解密后才创建系统密钥并重写。`save` 和 `save_merged_preferences` 只写版本 2。

- [x] **步骤 4：运行配置专项测试**

运行：`cargo test --quiet session::config::tests`

---

### 任务 4：启动与错误诊断逻辑

**文件：**

- 修改：`src/app/startup.rs`
- 修改：`src/app/mod.rs`
- 修改：`src/app/ui.rs`
- 修改：`src/sftp/mod.rs`
- 修改：`locales/en.yml`
- 修改：`locales/zh-CN.yml`

**接口：**

- `StartupConfig::load() -> StartupConfig` 返回配置和可显示错误
- `Ashell::new(window, cx, startup_config)` 消费已加载配置
- `ConfigStore::default_tmp_dir() -> PathBuf` 不依赖加密配置加载

- [x] **步骤 1：编写运行逻辑失败测试**

覆盖加载失败进入 `is_persistent() == false` 的内存模式、错误文本保留、SFTP 默认临时目录无需加载配置。

- [x] **步骤 2：运行测试并确认现有重复加载或 `unwrap` 路径仍存在**

运行：`cargo test --quiet startup_config`

- [x] **步骤 3：实现单次加载和可见通知**

`open_main_window` 加载一次并传给 `Ashell`；首次渲染用 `Notification::error` 显示本地化提示。删除 SFTP 编辑路径中的 `ConfigStore::load().unwrap()`。

- [x] **步骤 4：检查敏感日志和运行回归**

运行：

```text
rg -n "master_key|as_bytes\(\)|payload|password" src/session/config_key.rs src/session/config.rs src/app/startup.rs
cargo test --quiet
```

---

### 任务 5：文档与完整验证

**文件：**

- 修改：`AUDIT_REPORT.md`
- 修改：`docs/superpowers/specs/2026-07-29-config-credential-protection-design.md`

- [x] **步骤 1：更新中文审计状态**

将机器标识派生密钥问题标为已修复、待确认提交；说明版本 1 代码仅用于一次性迁移，并记录测试数量。

- [x] **步骤 2：执行完整验证**

运行：

```text
cargo fmt --check
cargo test --quiet
cargo check --all-targets
git diff --check
git diff --name-only -- .github scripts
git status --short --branch
```

预期：全部检查通过，`.github` 和发布脚本没有变化，工作区未暂存且没有新提交。

# 配置原子保存实施计划

> **执行要求：** 使用 `superpowers:executing-plans` 在当前会话逐项执行；步骤使用复选框跟踪。

**目标：** 将版本 2 配置的直接覆盖写入替换为可同步、可诊断的同目录原子写入。

**架构：** 在 `session::config` 内新增单一原子写入函数，使用 `tempfile::NamedTempFile` 完成暂存和跨平台覆盖。两个配置保存入口只负责生成加密字节并调用该函数。

**技术栈：** Rust 2024、`tempfile 3`、标准文件同步 API、现有 anyhow 错误链。

## 全局约束

- 替换失败时保留旧目标文件字节。
- 临时文件必须与目标配置位于同一目录。
- 日志和错误不得包含密钥或配置明文。
- 不修改发布流程，不暂存，不提交。

---

### 任务 1：原子写入原语

**文件：**

- 修改：`Cargo.toml`
- 修改：`Cargo.lock`
- 修改：`src/session/config.rs`

**接口：**

- `atomic_write_config(path: &Path, bytes: &[u8]) -> Result<()>`
- `atomic_write_config_with(path, bytes, persist)` 供生产逻辑和失败注入测试共享

- [x] **步骤 1：编写失败测试**

覆盖已有文件成功替换，以及注入替换错误时旧文件字节不变。

- [x] **步骤 2：运行测试并确认接口缺失**

运行：`cargo test --quiet atomic_config_write`

- [x] **步骤 3：实现分阶段写入和原子替换**

同目录创建 `NamedTempFile`，执行 `write_all`、`flush`、`sync_all`、`persist`，并在 Unix 同步父目录。

- [x] **步骤 4：运行原子写入专项测试**

运行：`cargo test --quiet atomic_config_write`

---

### 任务 2：接入配置保存入口

**文件：**

- 修改：`src/session/config.rs`

**接口：**

- `ConfigStore::save()`
- `ConfigStore::save_merged_preferences(...)`

- [x] **步骤 1：确认现有版本 2 保存测试覆盖两个入口**

运行：`cargo test --quiet config_store_encryption` 和 `cargo test --quiet save_merged_preferences`。

- [x] **步骤 2：将两个 `fs::write` 调用替换为统一原子写入函数**

保留内存配置空路径短路行为，删除重复权限设置代码。

- [x] **步骤 3：运行配置专项回归**

运行：`cargo test --quiet session::config::tests`。

---

### 任务 3：完整验证与审计记录

**文件：**

- 修改：`AUDIT_REPORT.md`

- [x] **步骤 1：更新中文审计状态和测试数量**

将“配置文件保存不是原子操作”标记为已修复、待确认提交。

- [x] **步骤 2：执行完整验证**

运行：

```text
cargo fmt --check
cargo test --quiet
cargo check --all-targets
git diff --check
```

确认发布文件无变化且工作区没有暂存或新提交。

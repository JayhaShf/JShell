# SSH 主机密钥校验实施计划

> **执行要求：** 使用 `superpowers:executing-plans` 在当前会话按任务执行；所有步骤使用复选框跟踪。

**目标：** 为终端 SSH 和 SFTP 连接增加统一、严格的 OpenSSH `known_hosts` 主机密钥校验。

**架构：** 新增 `session::host_keys` 模块，显式解析 `<用户目录>/.ssh/known_hosts` 并调用 `russh_keys::check_known_hosts_path`。终端 SSH 与 SFTP Handler 携带目标 host/port，并在认证前调用同一个校验函数。

**技术栈：** Rust 2024、`russh 0.49`、`russh-keys 0.49`、`anyhow`、内置 Rust 测试框架。

## 全局约束

- 本轮仅修复 P0 主机密钥校验，不修改 Release 工作流。
- 未知主机、密钥变化、文件解析失败都必须拒绝连接。
- 不自动写入 `known_hosts`，不提供跳过校验的开关。
- Windows、macOS 和 Linux 都使用 `<用户目录>/.ssh/known_hosts`。
- 所有修改先保留在工作区，不创建 Git 提交。

---

### 任务 1：共享主机密钥校验模块

**文件：**

- 新建：`src/session/host_keys.rs`
- 修改：`src/session/mod.rs:1-4`
- 测试：`src/session/host_keys.rs` 内的 `#[cfg(test)]` 模块

**接口：**

- 提供：`pub fn verify_server_key(host: &str, port: u16, key: &PublicKey) -> anyhow::Result<()>`
- 内部提供：`fn verify_server_key_at_path(host: &str, port: u16, key: &PublicKey, path: &Path) -> anyhow::Result<()>`
- 依赖：`directories::BaseDirs` 和 `russh_keys::check_known_hosts_path`

- [ ] **步骤 1：编写失败测试**

在 `src/session/host_keys.rs` 中先建立测试模块，使用两个有效 Ed25519 公钥夹具和临时 `known_hosts` 文件，覆盖：匹配密钥成功、未知主机失败、同算法密钥变化失败、非默认端口匹配成功。

核心断言：

```rust
assert!(verify_server_key_at_path("example.test", 22, &known_key(), &path).is_ok());
assert!(verify_server_key_at_path("unknown.test", 22, &known_key(), &path).is_err());
assert!(verify_server_key_at_path("example.test", 22, &changed_key(), &path).is_err());
assert!(verify_server_key_at_path("example.test", 2222, &known_key(), &path).is_ok());
```

- [ ] **步骤 2：运行测试并确认失败原因**

执行：

```text
cargo test --quiet session::host_keys::tests
```

预期：因 `verify_server_key_at_path` 尚未实现而编译失败。

- [ ] **步骤 3：实现最小校验逻辑**

实现要求：

```rust
pub fn verify_server_key(host: &str, port: u16, key: &PublicKey) -> Result<()> {
    let home = BaseDirs::new().context("could not determine user home directory")?;
    let path = home.home_dir().join(".ssh").join("known_hosts");
    verify_server_key_at_path(host, port, key, &path)
}
```

`verify_server_key_at_path` 必须把 `Ok(true)` 转为成功，把 `Ok(false)` 转为包含 host、port 和文件路径的未知主机错误，并为 `KeyChanged` 或解析错误补充上下文。

- [ ] **步骤 4：导出模块并验证测试通过**

在 `src/session/mod.rs` 增加 `pub mod host_keys;`，执行：

```text
cargo test --quiet session::host_keys::tests
```

预期：4 个主机密钥测试全部通过。

---

### 任务 2：终端 SSH 接入严格校验

**文件：**

- 修改：`src/backend/ssh.rs:282-318`
- 修改：`src/backend/ssh.rs:756-768`

**接口：**

- 消费：`session::host_keys::verify_server_key`
- 修改：`ClientHandler` 从单元结构体变为携带 `host: String` 和 `port: u16` 的结构体

- [ ] **步骤 1：编写失败测试**

在现有 `src/backend/ssh.rs` 测试模块中增加结构测试，构造 `ClientHandler::new("example.test", 2222)` 并断言保存的 host/port。此测试会固定 Handler 必须携带连接目标，而不是继续作为无状态全信任结构体。

- [ ] **步骤 2：运行测试并确认失败原因**

执行：

```text
cargo test --quiet backend::ssh::command_history_tests::client_handler_keeps_target_identity
```

预期：因 `ClientHandler::new` 尚不存在而编译失败。

- [ ] **步骤 3：实现 Handler 接入**

增加构造函数：

```rust
impl ClientHandler {
    fn new(host: &str, port: u16) -> Self {
        Self { host: host.to_string(), port }
    }
}
```

连接时传入 `ClientHandler::new(&session.host, session.port)`。`check_server_key` 调用共享校验函数，成功后返回 `Ok(true)`，任何错误通过 `?` 中止握手。

- [ ] **步骤 4：验证终端 SSH 测试**

执行：

```text
cargo test --quiet backend::ssh::command_history_tests
```

预期：原有命令历史测试与新增 Handler 测试全部通过。

---

### 任务 3：SFTP 接入同一校验策略

**文件：**

- 修改：`src/sftp/mod.rs:938-951`
- 修改：`src/sftp/mod.rs:1940-1952`

**接口：**

- 消费：`session::host_keys::verify_server_key`
- 修改：`SftpClientHandler` 从单元结构体变为携带 `host: String` 和 `port: u16` 的结构体

- [ ] **步骤 1：编写失败测试**

在 `src/sftp/mod.rs` 的现有测试模块中增加 `sftp_handler_keeps_target_identity`，构造 `SftpClientHandler::new("files.example.test", 2222)` 并断言 host/port。

- [ ] **步骤 2：运行测试并确认失败原因**

执行：

```text
cargo test --quiet sftp::handle_tests::sftp_handler_keeps_target_identity
```

预期：因 `SftpClientHandler::new` 尚不存在而编译失败。

- [ ] **步骤 3：实现 SFTP Handler 接入**

连接时传入 `SftpClientHandler::new(&session.host, session.port)`。`check_server_key` 调用与终端 SSH 完全相同的 `verify_server_key`，成功后返回 `Ok(true)`。

- [ ] **步骤 4：验证 SFTP 测试**

执行：

```text
cargo test --quiet sftp::handle_tests
```

预期：现有 SFTP Handle 测试与新增 Handler 测试全部通过。

---

### 任务 4：整体回归验证

**文件：**

- 验证：`src/session/host_keys.rs`
- 验证：`src/backend/ssh.rs`
- 验证：`src/sftp/mod.rs`

- [ ] **步骤 1：运行格式化**

执行：`cargo fmt`

- [ ] **步骤 2：运行完整测试**

执行：`cargo test --quiet`

预期：全部测试通过，无失败测试。

- [ ] **步骤 3：运行全目标编译检查**

执行：`cargo check --all-targets`

预期：退出码为 0。

- [ ] **步骤 4：检查差异范围**

执行：

```text
git diff --check
git diff -- src/session/host_keys.rs src/session/mod.rs src/backend/ssh.rs src/sftp/mod.rs
```

预期：不存在空白错误，且没有 `.github`、发布脚本或其他 P1 修改。

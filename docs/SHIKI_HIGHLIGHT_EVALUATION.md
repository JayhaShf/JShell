# Shiki 与语法着色技术评估

评估日期：2026-07-30

## 1. 结论

不使用 Shiki 替换当前终端关键词高亮，也不为文本编辑器引入 Shiki。

- 终端继续使用原生 Rust 的关键词、状态码和 URL 匹配器。
- 文本编辑器继续使用 `gpui-component` 已集成的增量 Tree-sitter 着色器。
- 本轮不新增 JavaScript、WASM、Node/WebView、`syntect` 或 `two-face` 依赖。

这不是能力取舍，而是运行模型取舍。Shiki 适合 JavaScript/TypeScript 环境中的已知语言代码块；JShell 的终端是原生 Rust/GPUI 的动态 cell 网格，二者的数据模型和运行时边界不一致。

## 2. 当前项目现状

### 终端

`src/terminal/highlight.rs` 接收当前可见的 `RenderCell`，按行建立 UTF-8 字节到终端列的映射，再输出 `(row, col) -> Hsla`。它处理的是混合终端内容中的日志等级、运行状态、网络状态、HTTP 状态码、地址和 URL，不是假定单一编程语言的源码。

`src/terminal/mod.rs` 已按完整 cell 快照缓存结果；内容不变时不会重复扫描。后续若要优化，应把每行的小写转换移出逐关键词循环，或使用 Aho-Corasick/`RegexSet` 批量匹配，并优先只重算脏行。

### 文本编辑器

项目已经具备语法着色，无需等待后续接入：

- `Cargo.toml` 已为 `gpui-component` 启用 Bash、C/C++、CSS、Diff、Go、HTML、JavaScript、Lua、Make、Markdown、Python、Rust、SQL、TOML、TypeScript 和 YAML 等 Tree-sitter 功能。
- `src/document/language.rs` 根据文件名、扩展名和 shebang 选择语言。
- `src/document/controller.rs` 在文件加载完成后调用 `editor.set_highlighter(...)`。
- `gpui-component` 的 `SyntaxHighlighter` 保存语法树、支持增量编辑、解析超时和后台更新，并直接输出编辑器需要的字节范围样式。

因此，“各种语言对应的语法着色器”在普通远程文本编辑器路径中已经存在。后续工作的重点应是补充语言映射、主题验证和大文件只读模式的策略，而不是更换引擎。

## 3. Shiki 不适合终端热路径的原因

1. **运行时不匹配**：Shiki 4.3.1 是 TypeScript/ESM 库。`onig.wasm` 只提供正则引擎，TextMate grammar、主题、token 状态机和模块加载仍由 JavaScript 执行。
2. **嵌入成本过高**：纯 Rust 桌面应用需要额外引入 JS runtime 或 Node sidecar，并处理 ESM、WASM、语言/主题资产、线程隔离、崩溃恢复和 Rust-JS 数据桥接。
3. **终端不是追加式源码**：终端会发生回车覆写、光标移动、清屏、滚动、回流和任意 cell 更新。Shiki stream 面向按行追加的代码或 LLM 输出，无法直接表达这些 VT 行为。
4. **坐标模型不同**：Shiki token 使用 JavaScript 字符串偏移；终端渲染需要 UTF-8 字节、grapheme、宽字符和 cell 列之间的稳定映射。
5. **语义目标不同**：Shiki 需要确定语言；终端同一屏会混合 shell prompt、命令、日志、程序输出和 TUI。现有高亮表达的是跨语言运维语义。
6. **实时性能缺少依据**：Shiki 官方建议把 CPU 密集型高亮放入 Worker，但没有针对原生终端逐帧输入延迟的保证。

## 4. 备选方案

| 方案 | 终端关键词 | 文本编辑器 | 当前建议 |
|---|---:|---:|---|
| 现有 Rust 匹配器 | 最适合 | 不适合完整语法 | 保留并按需优化 |
| 项目现有 Tree-sitter | 不适合混合输出 | 已集成且支持增量 | 编辑器首选 |
| `syntect` 5.3.0 | 可处理稳定代码行，但语言识别困难 | 可用 | 当前无引入必要 |
| `two-face` 0.5.1 | 同 `syntect` | 扩充 grammar/theme 资产 | 仅在现有语言覆盖不足时评估 |
| Shiki 4.3.1 | 运行时和终端模型不匹配 | 可实现但重复现有能力 | 不采用 |

## 5. 后续建议

1. 为 `src/document/language.rs` 增加用户实际需要但当前缺失的扩展名映射，并同步启用对应 Tree-sitter feature。
2. 为普通文件验证语言切换、主题切换、编辑后增量着色和保存流程。
3. 单独评估大文件分页查看器是否需要“当前块只读着色”，不要复用可编辑文档的完整解析策略。
4. 若终端关键词高亮出现性能问题，先建立基准，再优化逐行小写分配和多关键词扫描；不以引入 Shiki 作为性能修复。

## 6. 资料

- [Shiki 仓库](https://github.com/shikijs/shiki)
- [Shiki 安装与运行时](https://shiki.style/guide/install)
- [Shiki 正则引擎](https://shiki.style/guide/regex-engines)
- [Shiki 性能建议](https://shiki.style/guide/best-performance)
- [Shiki Stream](https://shiki.style/packages/stream)
- [Shiki ANSI 支持](https://shiki.style/languages#ansi)
- [gpui-component SyntaxHighlighter](https://github.com/longbridge/gpui-component/blob/ca85b5588f5feea021f1ee86cb13703dc6f30ec4/crates/ui/src/highlighter/highlighter.rs)
- [syntect](https://github.com/trishume/syntect)
- [two-face](https://docs.rs/two-face/0.5.1/two_face/)
- [Tree-sitter 语法着色](https://tree-sitter.github.io/tree-sitter/3-syntax-highlighting.html)

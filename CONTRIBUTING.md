# Contributing to Daw

欢迎参与 Daw！Daw 的定位是开源的 Data Agent Workstation——默认即用，改一份 `brand.json` 就是你自己专属的工作台。

## 提 Issue

- 先用关键词搜一下仓库,确认没有重复讨论。
- Bug 报告请附:平台(Windows/macOS/Linux)、复现步骤、日志面板截图(应用右下角日志抽屉)。
- 功能建议请描述使用场景与预期交互,而不是只给一句话想法。

## 提 PR

1. 从 `main` 拉新分支,一个 PR 只做一件事。
2. 遵循现有约定:
   - **注释与 commit 用中文**,代码标识符用英文。
   - 前端:组件放 `src/components/`,跨组件状态放 `src/lib/`(如 `brand.ts`、`theme.ts` 的 signal 模式)。
   - 后端:命令进 `commands.rs` 并在 `lib.rs` 注册;领域工具放 `src-tauri/src/skill/` 对应模块。
   - 品牌相关文案一律走 `~/.daw/brand.json`(见 `src-tauri/src/brand.rs`),不要在组件里硬编码产品名。
3. 改动涉及前后端契约(事件名、命令参数/返回结构)时,两边同步改,并在 PR 描述里标注。
4. 保证 `npm run build` 与 `cargo test` 通过。
5. 首次贡献加一条 CHANGELOG 说明(如果文件不存在,可以顺手新建 `CHANGELOG.md` 记录本次变更)。

## 开发环境

```bash
npm install
npm run tauri dev      # 启动桌面应用(前端 + Rust 后端热重载)
cargo test             # 后端单测(src-tauri 目录下)
```

开发期间的本地数据在 `~/.daw/`(旧版 `~/.aioa` 首次启动会自动迁移),不要提交。

## 设计原则

- **Agent 与用户数据隔离**:系统提示词永远把「可用表/知识」当作外部输入注入,不在代码里写死任何业务库名或凭据。
- **配置即说明书**:`brand.json` 等用户配置要自带模板与注释文档(README 字段表),删掉也能回落到默认。
- **失败不阻断**:品牌/设置文件解析失败时用默认值兜底,不弹错误阻止启动。
# 发布与更新手册

Daw 的发版流程与 lakemind 同构：**GitHub Actions 构建并发布 GitHub Release**，
安装包走 Release 资产直链，`updates/latest.json` 是应用内更新器的唯一数据源。

## 概念速览

| 事物 | 位置 | 说明 |
| --- | --- | --- |
| 安装包 + 签名 | GitHub Release 资产 | 每个平台一个安装包 + 一个更新包（`.sig` minisign 签名由 CI 自动生成） |
| 更新 manifest | `updates/latest.json`(仓库 main 分支) | 发版 CI 自动生成并推回 main；应用从 `raw.githubusercontent.com/tsingliuwin/daw/main/updates/latest.json` 读取 |
| 更新说明 | `CHANGELOG.md` 对应小节 | manifest 生成时按版本号提取 |
| 签名密钥 | `~/.tauri/daw.key`(本机)+ GitHub secrets | 丢失后已发布版本将无法再验证后续更新包 |

## 发版步骤

1. **版本号四面同步**（同一个 commit 里改）：
   - `package.json` → `version`
   - `src-tauri/tauri.conf.json` → 顶层 `version`
   - `src-tauri/Cargo.toml` → `version`
   - `src-tauri/Cargo.lock` → `daw` 包的 `version`（跑一次 `cargo check` 也会自动更新）
2. **写 CHANGELOG**：在 `CHANGELOG.md` 顶部加 `## [X.Y.Z] - YYYY-MM-DD` 小节（新增/变更/修复/移除），这段文字就是应用内更新弹窗展示的内容。
3. **提交 + 打注解 tag**：
   ```bash
   git commit -am "chore: release vX.Y.Z"
   git tag -a vX.Y.Z -m "Release vX.Y.Z"
   git push origin main --follow-tags
   ```
   tag 推送即触发 `release.yml`。
4. **等 CI 完成**：GitHub Release 出现安装包资产；`updates/latest.json` 由机器人 commit（`chore: release vX.Y.Z [skip ci]`）推回 main。
5. **验证**：把 `updates/latest.json` 的 URL 在浏览器打开确认平台齐全；本地跑旧版本应用看能否收到更新提示。

## 构建范围

每次发布全平台构建：Windows（NSIS 安装包）、macOS arm64 与 x64（dmg + `.app.tar.gz` 更新包）、Linux（AppImage）。构建矩阵在 `release.yml` 里**静态写死**——历史教训：不要改回「prepare 输出 JSON 矩阵 → fromJson」的动态方案，`GITHUB_OUTPUT` 传复杂 JSON 曾导致 build 任务整体被跳过且不报错。

## 缓存机制

CI 有两个工作流，靠 `swatinem/rust-cache@v2` 的 **`shared-key: "tauri-<os>-<target>"`** 打通：

- `ci.yml`（缓存预热）：锁文件变化时或手动触发，把各平台 release 依赖编译产物热进缓存；
- `release.yml`（发版）：用相同 shared-key 命中缓存，跳过 DuckDB 等重型依赖的重复编译。

两者 shared-key 必须严格一致，改动时两个文件同步改。

## 签名密钥管理

- 生成：`npm run tauri -- signer generate -w ~/.tauri/daw.key -p "<密码>"`，把打印的公钥与 `.pub` 文件内容核对后写入 `src-tauri/tauri.conf.json` 的 `plugins.updater.pubkey`（该字段是 `.pub` 文件的原始内容）。本项目使用的密码保存在本机 `~/.tauri/daw.key.password`。
- GitHub secrets（repo → Settings → Secrets）共两个：`TAURI_SIGNING_PRIVATE_KEY` = `~/.tauri/daw.key` 文件内容；`TAURI_SIGNING_PRIVATE_KEY_PASSWORD` = `~/.tauri/daw.key.password` 文件内容。
- **私钥与密码绝不进仓库**（`.gitignore` 已忽略 `*.key`/`*.pem`）。
- 轮换：生成新 key → 改 `tauri.conf.json` pubkey → 更新 secrets → **从轮换后的下一个版本起**新包才可验证。老版本客户端仍持旧公钥，轮换前发布的最后一版需保证其下载安装包可用（同一版本重发可以旧私钥重签，或干脆接受老客户端不可更新）。
- 丢失私钥：无法再给更新包签名，应用内更新停摆，只能重新发安装包让用户手动重装。

## 失败回滚

1. 构建/打包失败：`git tag -d vX.Y.Z && git push origin :refs/tags/vX.Y.Z`，若已自动建了 GitHub Release（draft 之外）删除该 Release，修复后重新打 tag。
2. manifest 写错：直接改 `updates/latest.json` 提交（机器人 commit 是普通 commit，可手动覆盖）。
3. 已发布但发现严重 bug：按正常流程发一个新版本（同级别即可），更新器会推送给所有客户端；不要试图改写已发布的旧包。

## 更新链路排查

- 应用内「检查更新」失败：先看 `updates/latest.json` 是否可达（`curl https://raw.githubusercontent.com/tsingliuwin/daw/main/updates/latest.json`）；再看该文件 `platforms.windows-x86_64.url` 指向的 Release 资产是否存在、`.sig` 内容是否与资产配套。
- 用户侧提示签名不通过：多半是 secrets 与 pubkey 不匹配（换过 key 但 `tauri.conf.json` 没同步）。
- 国内网络访问 raw.githubusercontent.com 不稳时：换自定义域名只改 `plugins.updater.endpoints` 的 URL（保持 URL 路径返回同样的 JSON 结构即可），已安装旧版本客户端不需要任何变更。
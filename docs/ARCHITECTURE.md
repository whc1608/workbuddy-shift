# 架构

本文只讲仓库的分层、依赖方向与目录职责，用于回答「这段逻辑应该写在哪一层」。
产品功能见 [README.md](../README.md)，构建与发布命令见 [DEVELOPMENT.md](DEVELOPMENT.md)，
内置会话悬浮窗的模块约束见 [companion.md](companion.md)，工程约定与验证入口见
[AGENTS.md](../AGENTS.md)。

## 三宿主与依赖方向

同一份 `wb-shift-core` 被三种宿主复用：宿主只做协议适配，不含业务规则。

| 宿主 | 目录 | 入口 | 调用链 |
| --- | --- | --- | --- |
| 桌面 App（Tauri 2） | `src-tauri/` | `src-tauri/src/lib.rs` 的 `run()` | 前端 `invoke` → Tauri command → core |
| WebUI（本地 HTTP） | `crates/wb-shift-server/` | `src/main.rs` 的 `serve()` | 浏览器 `fetch` → axum 路由 → core |
| CLI | 同上（同一个二进制） | `src/main.rs` 的 `main()` | 终端参数 → core |

```
src-tauri/src/commands/（按业务域拆分的 Tauri command） ─┐
                                                       ├─► wb-shift-core（纯 Rust）
wb-shift-server/src/api/handlers/（按业务域拆分的 axum handler）┘
```

`wb-shift-core` 只依赖通用库（reqwest / rusqlite / tokio 等），不依赖 tauri、axum，
也不依赖前端 `dist/`。

- `wb-shift-core` 是唯一业务层：`crates/wb-shift-core/src/lib.rs` 只导出 `modules`，
  具体能力按业务域放在 `crates/wb-shift-core/src/modules/`。
- 两个宿主只做「参数解析 + 结果/错误字符串化」，且保持同源：
  `src-tauri/src/commands/`（Tauri IPC）与 `crates/wb-shift-server/src/api/handlers/`（REST）
  按业务域一一对应，覆盖同一批能力。新增能力时三处一起改：core 实现、两个宿主包装、
  前端路由表；`node scripts/check-command-routes.mjs` 在 CI 里校验三方名字/路径一致。
- 宿主内部结构：`commands/mod.rs` 与 `api/mod.rs` 只做模块声明与 re-export，
  `commands/<域>.rs` / `api/handlers/<域>.rs` 放薄包装；router 只在
  `api/router.rs` 装配一次。命令名、HTTP 路径与返回契约保持不变。
- 请求体是宿主层的协议契约：高风险接口在 `api/dto.rs` 定义明确类型（`#[serde(rename_all =
  "camelCase")]`，字段名与 Tauri command 参数逐字一致），失败统一成 400 + `{ok:false,error}`。
  仍是 `Json<Value>` 的 handler 登记在 `scripts/api-contract-baseline.json` 并写明理由，
  `node scripts/check-api-contract.mjs` 校验「DTO ↔ Tauri 参数」并拒绝未登记的新弱类型边界。
  响应体不需要两份定义：两个宿主都直接返回 core 的 `serde_json::Value`，形状天然同源。
- 前端不共享 Rust 类型：契约在 `src/lib/types/`，双通道适配在 `src/lib/api/`。
- 长任务状态归任务所有：`POST /api/switch` 的运行标记由 `spawn_blocking` 内部守卫清除，
  请求 future 被取消（浏览器断开）不会遗留「永久运行中」，后台仍在写数据时也不会放行第二次
  切换（见 `crates/wb-shift-server/src/api/state.rs` 的 `SwitchState` / `SwitchRunGuard`）。

## 六个 crate

Workspace 成员与共享依赖版本见根 [`Cargo.toml`](../Cargo.toml)。

| crate | 产物 | 职责 | 入口 |
| --- | --- | --- | --- |
| `wb-shift-core` | 库 | 全部业务逻辑（账号 / 会话 / 统计 / 限额 / 更新等） | `src/lib.rs`、`src/modules/mod.rs` |
| `wb-shift-server` | 二进制 `workbuddy-shift` | WebUI：axum API + `rust-embed` 嵌入 `dist/`；同时是 npm 安装形态的 CLI | `src/main.rs`、`src/api/` |
| `workbuddy-shift-desktop`（`src-tauri`） | 桌面 App（lib `wb_shift_lib`） | Tauri 宿主：commands、托盘、更新、单实例锁、悬浮窗宿主边界 | `src/lib.rs` |
| `companion-core` | 库 | 悬浮窗的会话监控与状态归一化（各客户端 hook / 日志解析） | `src/lib.rs`、`src/adapters/` |
| `companion-runtime` | 二进制 `companion-runtime` | 悬浮窗本地监控服务与 hook 客户端，作为 Tauri `externalBin` 打包 | `src/main.rs` |
| `companion-desktop` | Tauri 插件库 | 悬浮栏 / 设置窗口、命中测试、位置持久化 | `src/lib.rs` |

要点：

- `wb-shift-core` 不依赖 Tauri 也不依赖 Axum；这两个重依赖只出现在宿主 crate。
- `companion-*` 三件套与 `wb-shift-core` 互不依赖，只通过 `src-tauri` 装配进来
  （`src-tauri/src/companion.rs` 给插件传资源根与运行时路径）。
- `companion-desktop` 的 `links = "tauri-plugin-companion"` 决定 Tauri 权限命名空间，
  改名时要同步 capability 与前端 `plugin:companion|` 前缀，见 [companion.md](companion.md)。
- 宿主与 companion 也已按职责目录化：`src-tauri/src/tray/`（`menu`/`icons`/`platform`/`actions`）、
  `src-tauri/src/update_service/`（`state`/`download`/`install`/`tray_bridge`）、
  `companion-desktop/src/plugin/`、`companion-desktop/src/rail_settings/autostart/`（三平台子模块）、
  `companion-core/src/adapters/{workbuddy,codeg/{events,child}}/`、`companion-core/src/custom/engine/`；
  入口文件只做装配，事件/命令/持久化契约未变。

### wb-shift-core 模块地图

| 主题 | 模块（`src/modules/` 下） |
| --- | --- |
| 账号与切换 | `account/`（`store`/`display`/`identity`）、`auth_file/`（`read`/`sync`）、`switch/`（`plan`/`execute`）、`variant.rs`（档位单一来源）、`oauth.rs`、`export_import/`（`export`/`import`）；`process/`（`mod.rs` 公共入口 + `windows/`（进程/注册表/解析）+ `win_close.rs`（Windows 可见窗口 `SC_CLOSE` 投递与窗口标题探测，当前由 `vscode_ext` 使用，二期接入其余客户端）+ `macos.rs`） |
| 会话复制 / 同步 / 恢复 | `session/mod.rs`（公共入口与共享口径）、`session/paths.rs`（存储/数据根与命名空间）、`session/query.rs`（会话列表、DB 探测与会话行存活索引 `LiveSessionRows`）、`session/copy/{mod,body,db,db_spaces,plan,plan_prepare,plan_execute,repair}.rs`、`session/exec/{mod,backup,write,write_replay}.rs`、`session/sync/{mod,preview,liveness,plan,wait}.rs`、`session/recovery.rs`、`session/run.rs`、`session/tests/`、`session_backup.rs`、`session_groups/`（`view` 列表/详情、`liveness` 插件/IDE 的组级存活汇总与清理拒绝原因、`sync` 配对、`sync_all` 跨组安全批量（一次确认同步所有可安全快进的副本，分歧组整组跳过、编辑器只关/开一次）、`members` 成员操作、`graph` 统一批次、`maintenance` 失效组清理、`jetbrains_guard` 切换前的组一致性守卫） |
| 会话关联存储与判定 | `session_link.rs`（模块声明与 re-export）与其子模块 `session_link/{content,content_repair,link_store,link_store/{baseline,store},operation_log,preview,sync_decision}.rs`、`session_link/tests/` |
| 其它客户端 | `codebuddy_cli/`（`state`/`settings`/`helper`/`switch`）、`codebuddy_ide/`、`codebuddy_cn_ide/`、`codebuddy_ide_session/`、`codebuddy_ide_session_sync/`、`vscode_ext/`、`vscode_session/`、`vscode_session_sync/`（`index` 工作区索引四态/定位/会话目录证据、`liveness` 会话存活判定（三处消费方共用）、`register` 复制后登记、`preview` 预览、`plan`/`exec` 同步、`backup` 回滚）、`vscode_session_link/`、`vscode_cn_inject/`、`jetbrains/`（`mod/status/switch/detect/...`，`diagnose` 输出切换前诊断与阻断原因） |
| 存储查看与清理 | `storage/`（`catalog` 分类聚合、`snapshot` 单次遍历快照（总览/会话/回收站共用）、`protected` 受保护目录判定与延迟计量、`reveal` 文件管理器定位（`scope=storage` 白名单 / `scope=cwd` 只读任意绝对目录）、`roots/` 白名单根） |
| 限额 | `limits/`（`scan` 日志扫描、`parse/{mod,workbuddy,ide_cli}` 解析分类、`ledger/{mod,cache,payload}` 台账与缓存、`payload` 输出、`vscode_ext` VS Code 插件日志能力探测（只诊断，不产出限额条目））、`rate_limit_events/`（`state`/`parse`/`watch`）、`rate_limit_hook/`（`install`/`uninstall`/`status`） |
| 统计与自动化 | `credits/`（`http` 取数、`resources` 投影）、`credit_usage/`（`collect`/`project`）、`official_usage/`（`fetch`/`parse`/`aggregate`）、`token_stats/`（`jsonl` 解码、`ide` 数据源、`projection` 聚合、`cache` 文件缓存）、`checkin/`（`status`、`window` 时间段、`cycle` 轮次、`logs`）、`travel/`（`status`/`cycle`/`config`）、`rotate/`（`decision`/`cycle`/`logs`）、`refresh.rs` |
| 基础设施 | `config/`（`paths` 路径与环境变量、`store` 原子写/通用 JSON、`http` 请求与错误归一化 + 共享 `is_unauthorized`/`parse_number`、`configs/{mod,checkin,travel,automation}` 业务配置）、`notifications.rs`、`session_backup/`（`backup`/`lifecycle`/`cleanup`）、`error_log.rs`、`diagnostics/`（`redact` 脱敏纯函数、`report` 报告装配与 `.txt` 落盘）、`update.rs`、`switch/`、`linux_keyring.rs`（仅 Linux）、`locks.rs`（进程内锁获取 helper：中毒返回错误或显式恢复） |

> `session` / `session_link` 已拆成目录模块（文件即子边界）；对外仍从 `session::` /
> `session_link::` 的 re-export 使用，调用方不需要跟着改路径。

会话「存活」判定有两套依据，按客户端分派，互不串用：

| 客户端 | 判定依据 | 实现 |
| --- | --- | --- |
| WorkBuddy | `workbuddy.db` 的 `sessions` 行（按成员档位读库） | `session::group_any_alive` |
| VS Code 插件 / CodeBuddy IDE | 工作区索引 `index.json` 条目**或**会话目录 `history/<workspaceHash>/<conversationId>/` | `vscode_session_sync::liveness`（`LivenessJudge` / `SessionLiveness`） |

插件侧的判定优先级是「索引命中 → 目录存在 → 两者都没有时看四态」：目录比索引条目更可靠，
`index.json` 损坏 / 半截写入 / 条目被改归属时条目会缺失，会话目录却还在。目录集合与定位表
来自**同一次**遍历（`conversation_index_scan` 进每个工作区目录时顺手 `read_dir`），
**不得**为每个待判定的会话单独 `is_dir()`。链接（符号链接 / junction）指向的目录同样算证据：
普通目录项走 `DirEntry::file_type()`，链接项补一次跟随判定。判定结果按 `(spec, root, uid)`
缓存，一个请求内一个账号只扫一遍盘；预览 / 管理页 / 批量清理三处共用同一份结论
（`session_groups::liveness` 做组级汇总与拒绝原因），所以**管理页隐藏的组 ≡ 清理能删的组**。

四态只回答一个问题——「不在索引里」能不能当作会话已消失：

| 状态 | 含义 | 索引与目录都没命中时 |
| --- | --- | --- |
| `Absent` | 历史目录不存在（账号从未用过 / 数据根不对） | `Unknown`：读路径不过滤，写路径拒绝整次清理 |
| `Unavailable` | 目录在，但一份 `index.json` 都没解析成功 | 同上 |
| `Partial` | 有索引文件的工作区里只有一部分解析成功：`ids` 只是**可读分片**的并集 | 同上（`Partial` 下命中可读分片即 `Alive`） |
| `Available` | 每一份索引都解析成功 | `Gone`：读路径过滤，写路径可删组 |

分母只数**含 `index.json` 的工作区目录**：目录在却没有索引文件通常是客户端正常状态
（空工作区 / 尚未写入），把它算进分母会让「有一个空工作区」就永远 `Partial`，该客户端的
清理被永久拒绝；那种现场若会话真的存在，目录证据会兜底。

`Partial` 是关键的一格：只读分片里没命中**不能**证明会话已消失（损坏的那份里可能正装着
它），所以两侧都向「不删 / 不隐藏」的方向取舍。读路径在非 `Available` 时 **fail-open**，写路径
在非 `Available` 时 **fail-closed**（`Partial` 的落点是「找到就活、找不到就判不了」）——破坏性
操作绝不「读不到就当全死」，但也不会因为一处损坏就永久挡住全部清理（会话目录存在即可正常
处理，这是 2026-11 那一轮的收益）。定位表、四态与目录证据来自同一次目录遍历
（`conversation_index_scan`），不要新增第二份扫描逻辑。

管理页按客户端分派：`Workbuddy` 用 `sessions` 行，插件 / IDE 用上面的组合判定；插件 / IDE
的数据根解析不到时 **fail-open** 全部展示（读路径不因缺依据而隐藏）。

`list` 载荷的清理入口状态有两个键（**都始终存在**）：`deadGroupCount`（被隐藏的失效组数）
与 `pruneBlocked`（清理会被拒绝的原因，无阻塞时是 `null` 而不是省略）。两者由
`view::list_payload` **一处**装配（`list_at_with_index_root` 与用例共用），并且与批量清理
**同一趟判定**产出：

- 主表可读：`blocked` 非空 ⇔ `prune_dead_groups` 会拒绝，文案逐字相同；
- 主表损坏（`Unavailable`）：`prune` 返回 `Err`，所以 `pruneBlocked` 是同一句
  「会话关联组暂不可用：…」（`store_unavailable_reason` 是唯一来源，`detail` / `members` /
  `sync` 的错误也共用它）；
- 主表不存在（`Missing`，首次使用）：`prune` 返回 0 组而**不是**拒绝，`pruneBlocked`
  必须保持 `null`。

因此前端可以把「可点但必失败」的入口改成「禁用 + 展示原因」，不会出现前后端判定不一致。
`list_at` 只替换关联存储路径（数据根按客户端解析），`list_at_with_index_root` 连插件 / IDE
的数据根一起注入：用例因此能覆盖 `Ready` 分支从扫描结论到载荷的最后一跳（数据根解析不到
即是阻塞），而卡片正文无法注入、只用「主表里没有组」的现场跑通，全程不读真实客户端数据。

## 前端分层

`src/` 是主界面（React 19 + Vite + Tailwind + shadcn/Radix），`companion-ui/` 是悬浮窗的
独立 Vite 工程，两者不共享代码。

| 目录 | 职责 |
| --- | --- |
| `src/main.tsx`、`src/App.tsx` | 挂载、全局错误处理、侧栏布局与 5 条懒加载路由；footer/更新中心在 `components/app-shell/` |
| `src/pages/` | 页面；页面目录内按区块拆文件：`sessions/hooks/{use-session-groups,use-session-detail,use-session-actions}.ts`、`accounts/hooks/use-accounts-page.ts`、`accounts/components/`、`settings/hooks/use-config-autosave.ts` 等 |
| `src/components/` | 跨页面组件；`ui/` 是 shadcn 基础件；`account-card/` 账号卡片区块；`client-switch-account-*` 两个客户端共享切号弹窗；`app-shell/` 是侧栏 footer 与更新中心 |
| `src/hooks/` | `use-*` 轮询/可见性/更新状态/悬浮窗开关；`use-visible-interval` 是可见性轮询的唯一实现 |
| `src/lib/` | 纯逻辑与契约：`api/`（双通道）、`types/`（按域拆分）、`screenshot-demo/`（按域 fixtures + 分发）、`persisted-preference.ts`（localStorage 偏好唯一实现）、主题/格式化等 |
| `src/stores/` | zustand 状态；目前只有 `accounts.ts`（账号库、状态快照、轮询去重） |

### API 双通道

`src/lib/api/` 按业务域拆文件，统一从 `src/lib/api/index.ts` 再导出；页面只
`import * as api from "@/lib/api"`。核心适配在 `src/lib/api/core.ts`：

- 桌面 App：`invoke(cmd, args)` 调 Tauri command。
- WebUI：按 `ROUTES` 表把同一个 `cmd` 映射成 HTTP 方法与路径，`fetch` 同源服务
  （端口 / SSH 转发都跟随页面来源，不写死 `127.0.0.1:57910`）。
- 运行形态判定：`isWebui()`（无 `__TAURI_INTERNALS__`）、`isDesktop()`、`isDemoMode()`。
- 新增能力时，Tauri command 名必须与 `ROUTES` 的键一致，否则 WebUI 会报
  「webui 模式暂不支持该操作」。`node scripts/check-command-routes.mjs` 校验
  Tauri 注册、command 定义、ROUTES、axum 路由四方一致；`node scripts/check-api-contract.mjs`
  校验请求体 DTO 与 Tauri 参数一致；`node scripts/check-companion-contract.mjs`
  校验悬浮窗演示桥的 postMessage channel 两端一致；`node scripts/check-structure.mjs`
  校验长文件豁免表、core 依赖方向、旧名称与 capability 最小化。
- **结构化失败**：`src/lib/api/failure.ts` 里的 `ApiFailure`（`extends Error`）负责把
  `{ code, message, failureKind, httpStatus }` 从两端还原出来——webui 取非 2xx 响应体，
  桌面端对 `invoke` 的 `Err(String)` 做 `JSON.parse`（`failure_response` 的序列化结果本身就是
  JSON）。**协议只有这四个字段**：「下一步动作」是界面决策，由前端 `ACTION_BY_KIND` 映射
  （`login_expired→relogin`、`gateway_html→check_proxy`、`malformed→copy_diagnostic`、
  `upstream→无`），不往传输层加字段。`asError()` 的返回值**逐字未变**（仍读 `message`），
  新增的 `asFailure()` 只服务需要动作的新代码；Tauri 侧只有真正解析出结构时才替换抛出值。
  注意：切号（本地文件操作，不经过 `http_request`）与 OAuth 的 `fail_oauth` 都**不产生四档**，
  那些界面上动作行只在真有 `failureKind` 时出现，否则整块不渲染。
- **积分链路走「结果字段」而不是「异常」**：`get_credit_expiry` 返回 `Value`（不是 `Result`），
  失败也是一个正常 resolve 的 `{ ok:false, error, failureKind?, httpStatus? }`。
  档位由 core 的 `credits::credit_failure_result`（`resources.rs`）从上游响应原样透传
  （没有就**不写这个键**，`error` 文案一字不改），前端由
  `src/pages/credit-stats/credit-failure.ts` 的 `findCreditFailure` 从 `creditMap` 里挑出来，
  交给 `failureFromResponse()` 还原成 `ApiFailure` 再渲染动作行。**这条路径不能用
  `asFailure()`**——那读的是 reject 值，这里没有 reject。
- `get_credit_expiry` 结果里的 `failureKind` / `httpStatus` 目前是**预留字段**（前端
  `CreditExpiry` 不消费，动作画面已改由 `officialUsage.errors[]` 承载），由
  `credits/tests/failure.rs` 守护形状。**不要因为「没人用」就删**——那是契约的加法式扩展，
  删掉以后想再用还得再改一次协议面。

### Demo Mode

只读演示跑的是同一份前端代码，数据由 `src/lib/screenshot-demo/`（按域 fixtures + 薄分发）
提供：

| 开关 | 作用 |
| --- | --- |
| `VITE_DEMO_MODE=1` | 只读演示：`demoModeEnabled` 生效，写操作统一抛「演示模式下不可操作」 |
| `VITE_SCREENSHOT_DEMO=1` | README 截图 / 演示构建，同样只读 |
| `VITE_PAGES_DEMO=1` | GitHub Pages 部署：改用 `HashRouter`、Vite `base=/workbuddy-shift/` |

`call()` 在演示模式下只放行 `DEMO_READ_COMMANDS` 白名单里的读命令，其余直接拒绝；
`pnpm run build:demo` 产出 Pages 用的静态站点（见 `.github/workflows/pages.yml`）。
`src/lib/demo-contract.test.ts` 校验白名单与演示分发不漂移；悬浮窗演示的 postMessage
channel 由 `scripts/check-companion-contract.mjs` 校验。

### 内置悬浮窗前端

`companion-ui/` 由 `scripts/build-companion.mjs` 驱动：嵌入包输出到
`public/companion/`（`desktop.html` + `desktop-settings.html`），运行时二进制输出到
`src-tauri/binaries/companion-runtime-<target>`。构建时机与目录细节见
[companion.md](companion.md)。

## 数据存储

工具自己的数据在 `~/.workbuddy-shift/`；开发构建把家目录整体重定向到 `~/.wb-shift-dev`，
存储根随之变成 `~/.wb-shift-dev/.workbuddy-shift/`（见下文「沙箱」）。客户端数据只读写
各自产品目录；路径常量集中在
`crates/wb-shift-core/src/modules/config/paths.rs` 与 `variant.rs`。

### 工具存储根（`~/.workbuddy-shift`）

| 文件 / 目录 | 内容 |
| --- | --- |
| `accounts.json` | 账号库（含凭据字段；原子写） |
| `backups/` | 切换前登录态备份 + 会话操作备份（见「备份与恢复」） |
| `auto_checkin_config.json`、`auto_checkin_logs.json`、`auto_travel_config.json`、`travel_cache.json` | 自动签到 / 自动旅行 |
| `auto_rotate_config.json`、`auto_rotate_logs.json`、`auto_rotate_notify.json` | CodeBuddy CLI 自动轮换 |
| `rate_limit_config.json`、`rate_limit_state.json`、`hook-events.jsonl` | 限额配置、入账状态、hook 事件追加文件 |
| `hook.sh`、`hook-backups/` | 限额 hook 脚本；安装前把客户端原配置备份到这里 |
| `credit_usage_snapshots.json`、`official_usage_cache.json` | 积分 / 官方用量缓存 |
| `workbuddy_exe.json`、`codebuddy_cn_app.json`、`codebuddy_ide_app.json` | 各客户端安装路径缓存 |
| `codebuddy_ide.json`、`codebuddy_cn_ide.json`、`vscode_ext.json`、`jetbrains.json` | 各客户端当前账号 / 切换状态 |
| `github_config.json`、`notifications.json`、`error.log` | 更新代理配置、提示存档、前端错误日志 |
| `session_links.json`、`session-links/` | WorkBuddy 关联组、基线、操作日志、预览凭据 |
| `vscode_session_links.json`、`vscode-session-links/` | VS Code 插件同构存储 |
| `codebuddy_ide_session_links.json`、`codebuddy-ide-session-links/` | CodeBuddy IDE 同构存储 |
| `locks/` | 跨进程锁：`session-ops-{variant}.lock` / `session-links.lock`（会话操作，顺序见 `session_link.rs` 文件头）、`credentials.lock`（凭据写入：切号写回 ↔ 令牌刷新回写同一把锁，见 `auth_file/sync.rs` 文件头） |

三个关联命名空间互不可见：文件名 / 目录名不同，避免 WorkBuddy、VS Code 插件、
CodeBuddy IDE 的关联关系互相污染（见 `session/paths.rs` 的 `LinkNamespace`）。

### 诊断包（排障用的一次性导出）

`crates/wb-shift-core/src/modules/diagnostics/` 把「作者必问的那几项」装配成一段脱敏文本
（版本 / OS-arch / 宿主形态 / 语言 / `error.log` 最近 20 条 / 设置项**键名 + 值类型** /
账号数量区间 / 各客户端装没装 / 工具数据目录状态（只读判断，不写盘）/ 限额 hook 安装状态），**不落任何自有
文件**：桌面端写到用户选的 `.txt`（`export_diagnostics`，非 ROUTES 路由），网页版由前端
复制或浏览器下载（`GET /api/diagnostics` 只返回文本）。

两条结构性约束（不是注释，是编译期事实）：

1. 模块树里**没有** `account` / `auth_file` / `oauth` / `session` 的 `use`——账号条数由宿主层
   以 `usize` 传入（只做区间分桶），客户端探测结果同理；因此「不小心把 `accounts.json` 的
   字段带出来」在类型层面就做不到；
2. 脱敏在 `redact.rs` 里是一组**纯函数**（`<path>` / `<url>` / `<email>` / `<token>` /
   `<uuid>` / `<num>` / `<user>` 七种占位符，风格与既有的 `limits::vscode_ext::redact` 对齐；
   URL 保留 `scheme://host` 便于判断是哪家网关，路径与 query 抹成 `<url>`），单测直接喂含
   真实令牌与邮箱的日志样本，断言输出里一个原文片段都不出现；另有顺序断言守住
   「UUID / 令牌先于长数字」（否则 `550e8400-…-446655440000` 会被数字规则拆碎）。

脱敏的三条边界（**不是 bug，别反复改**）：

- **`Bearer` 之后的短令牌一定抹**：长度规则只认 ≥32 字符，`Authorization: Bearer
  sk-secret-123` 里 13 字符的令牌靠「前一个词是 `Bearer`」的上下文规则认出来，抹成
  `<token>`；刻意**不**扩展到 `token=` / `access_token=` / `password=` 这类键值形态——
  那会把正常文案（`password=见文档`）一起吃掉。
- **`KEY=长令牌` 会连键名一起吞**：`GH_TOKEN=ghp_…` 整词变 `<token>`（`=` 是词字符）。
  这是「宁可比必要的更狠」的有意取舍（少一个键名只让作者少一个线索，反向漏一个令牌是事故）。
  同理，**含空格的路径**只保证「不留路径残片」：`/Users/a/Library/Application Support/WB/`
  会变成 `<path> <path>`（前一个词判成路径后，后续仍含 `/` 且长度 > 3 的片段继续抹），
  但路径之后**不含 `/` 的普通词**不会被抹——那是正常英文，抹掉只会让报告读不懂。
- **「工具数据目录可写」是疑似不是事实**：探针改成纯只读判断（`metadata` + `read_dir`，
  任何情况下都不创建 / 不修改 / 不删除文件，避免进程中途被杀留下垃圾文件）。POSIX 的
  mode 位判不了 root 与 ACL，Windows 上目录的 readonly 属性基本无意义，所以报告文案保留
  「可能」，不要改成断言句。

### 各客户端数据

| 客户端 | 读写的本机数据 |
| --- | --- |
| WorkBuddy 国内版 / 国际版 | 登录态 `…/CodeBuddyExtension/Data/Public/auth/workbuddy-desktop[-ai].info`；数据根 `~/.workbuddy` / `~/.workbuddy-ai`（会话正文 `projects/**/*.jsonl`、`workbuddy.db`、云端映射 `edge-sync-mapping*.db`、`logs/`） |
| CodeBuddy CLI | `~/.codebuddy/settings.json`、会话与日志目录；轮换状态 `~/.codebuddy-rotate/state.json` |
| CodeBuddy IDE（国内 / 国际） | 用户数据目录 `.../CodeBuddy CN` 或 `.../CodeBuddy` 下的 `User/globalStorage/state.vscdb`（Safe Storage secret），只读写登录态行 |
| VS Code（官方版） | `User/globalStorage/state.vscdb` 中 CodeBuddy 扩展的 secret；写入前要求 VS Code 退出 |
| JetBrains（16 个产品：IDEA / PyCharm / WebStorm / GoLand / CLion / PhpStorm / Rider / RustRover / RubyMine / DataSpell / DataGrip / Aqua / MPS / Writerside / Android Studio / AppCode（已停更）） | 双配置根：`.../JetBrains/<产品><版本>/` 与 `.../Google/<产品><版本>/`（Android Studio 落在后者）下的 `options/secret-storage.xml` 与已安装的 `plugins/coding-copilot*`；产品清单由 `jetbrains/products.rs` 单一来源派生 |

WorkBuddy 的会话复制需要「正文 + `workbuddy.db` sessions 表」同时可用；数据根不具备该
结构时按不支持处理（`session::session_copy_supported_at`），不硬编码按档位放行。

### 备份与恢复

- **登录态**：切换前把客户端登录态文件复制到 `backups/workbuddy-desktop[-ai].<时间>.info`。
- **会话复制**：在操作专属目录 `backups/session-transactions/<variant>/<operationId>/`
  逐文件复制 `workbuddy.db`（含 `-wal` / `-shm`）并校验大小；备份不可信就不写业务数据。
- **会话同步**：同一目录布局下用 rusqlite 在线备份 API 生成数据库一致性快照，逐文件备份
  目标正文并记录摘要，写 `manifest.json` 记录覆盖前正文 / 目标行 / 恢复方法；备份不可信时零写入。
- **生命周期**：维护记录 `session-links/backup-lifecycle/` 标记 `allocating → protected →
  cleanupPending`；备份不完整时零写入，中断的操作由 `session/recovery.rs` 按同一份清单
  补完或回滚，可靠完成后再清理临时目录（`session_backup.rs`）。
- **写入范围**：复制 / 同步只写目标正文与 `workbuddy.db` 的 sessions 行；
  `edge-sync-mapping-*.db` 的云端映射行由客户端 edge-sync 扩展在下次启动时自行写入，
  本工具不预写（预写会让客户端判定「已迁移」从而跳过上传）。
- **VS Code / CodeBuddy IDE 会话**：复制 / 同步前把索引与正文备份到
  `backups/{vscode-sessions,codebuddy-ide-sessions}/<utc>/`。
- **hook 配置**：安装限额 hook 前把三处客户端 `settings.json` 备份到 `hook-backups/`，
  卸载时在「除本工具条目外没有其它改动」的前提下逐字节还原。
- **hook 脚本归属**：安装 / 卸载只动「本工具生成的正文」——`Current`（当前模板）与
  `Outdated`（`$HOME` 形态的最早一版，以及**骨架一致、只有事件文件路径不同**的同代模板，
  例如上一代数据目录留下的那一份）；骨架认不出的一律按「用户改过」保留。历史注册条目
  （旧 `hook.cmd`、上一代数据目录里的同名脚本）在安装 / 卸载时按**具体绝对路径**认领并替换，
  不用宽泛片段认领第三方命令（旧名称例外登记见 `AGENTS.md` 第 7 节）。

### 加密（只描述现状）

- 工具**不加密**自己的 `accounts.json`；凭据字段来自客户端，WorkBuddy 5.6 的
  `$wbEncrypted` 加密信封原样保存、由客户端自行解密，本工具不降级成空串。
- 切到 CodeBuddy 系客户端时，按 Chromium / Electron Safe Storage 的既有模型写入目标
  自己的加密存储：macOS Keychain → PBKDF2-SHA1 → AES-128-CBC `v10`；Windows Local State
  + DPAPI → AES-256-GCM `v10`；Linux Secret Service → AES-128-CBC `v11`（无密钥环时退回
  peanuts 固定密钥）。实现集中在 `vscode_cn_inject.rs`，Linux 密钥环读取见
  `linux_keyring.rs`；工具不落盘明文主密码。
- 只读写目标客户端自己的数据库 / XML 字段，不做全库解密，也不导出解密后的凭据。

### 沙箱

`WB_SHIFT_HOME` 覆盖「家目录」，账号库、客户端数据、备份、hook 事件全部随之重定向。
开发构建（`tauri dev`）在未显式设置时默认指向 `~/.wb-shift-dev`，位置可用
`WB_DEV_SANDBOX` 指定；`WB_DEV_REAL_HOME=1` 才回到真实数据。实现见
`src-tauri/src/lib.rs` 的 `sandbox_dev_home()` 与 `config/paths.rs` 的 `dev_home_dir()`，使用说明见
[`scripts/README.md`](../scripts/README.md)。

## Tauri 2 权限与安全边界

权限按**实际窗口**最小化，capability 文件在 `src-tauri/capabilities/`：

| 窗口 | capability | 权限（最小集） | 依据 |
| --- | --- | --- | --- |
| `main` | `default.json` | `core:event:default`；`core:window:allow-is-visible`；`core:window:allow-start-dragging`；`core:window:allow-internal-toggle-maximize`；`opener:default`；`dialog:allow-save`；`dialog:allow-open` | 前端只监听事件、调 `isVisible()`、用 `data-tauri-drag-region`、`plugin-opener`/`plugin-dialog`；`dialog:allow-open` 只服务加密备份的「选文件导入」（需要绝对路径，`allow-save` 只管保存），读文件在 Rust 侧、不涉及 fs/shell/process；更新/通知由 Rust 内部完成，不需要前端 capability |
| `companion-rail`、`companion-settings` | `companion.json` | `core:event:default`；`core:window:allow-start-dragging`；`notification:allow-is-permission-granted`；`notification:allow-request-permission`；`companion:default` | companion-ui 只 listen 事件、调 `plugin:companion|` 命令、请求通知权限、拖动悬浮栏 |

- 不新增 fs / shell / process 等宽泛 capability；Rust 需要的能力（opener、autostart、单实例、更新）
  由 Rust API 直接调用，不暴露给前端。
- `app.security.csp` 是生产 CSP：`default-src 'self'`、`script-src 'self'`、允许内联样式、
  只放行 `ipc:` / `http://ipc.localhost` 连接、禁止 object/base/form 外发。
  `app.security.devCsp` 仅供 `tauri dev`（Vite HMR/React preamble），生产 CSP 不放宽。
  `dangerousDisableAssetCspModification: ["style-src"]` 只关闭 style-src 的 nonce/hash 注入
  （React 运行时设置 style 属性需要），其余指令仍由 Tauri 注入。
- companion 的窗口 label、capability identifier、插件名、invoke 前缀、事件名必须成组修改，
  见 [companion.md](companion.md) 的「运行时标识」表。
- WebUI（本地 HTTP）安全边界：只绑定 127.0.0.1；Host 校验防 DNS rebinding；Origin 校验防
  跨站 simple request（CSRF）；请求体上限 2 MiB；静态资源来自 `rust-embed` 编译期表
  （无磁盘路径、带 `X-Content-Type-Options: nosniff`）；未知 `/api/*` 返回 404 而不是
  SPA 回退。上述行为都有 `crates/wb-shift-server/src/api/tests.rs` 覆盖。

## 长文件与拆分计划

阈值（[AGENTS.md](../AGENTS.md)）：手写源码以 ~300 行为目标，超过 500 行必须拆分，或在本文登记理由与后续计划。

本轮（Round 3）已完成的拆分：

- `wb-shift-core` 的生产模块全部目录化：`session/`、`session_link/`（含 `tests/`）、`process/{windows/}`、`config/{paths,store,http,configs/}`、`limits/{scan,parse/,ledger/,payload}`、`rate_limit_events/`、`rate_limit_hook/`、`credits/`、`token_stats/`、`checkin/`、`session_groups/`、`session_backup/`、`travel/`、`account/`、`official_usage/`、`credit_usage/`、`rotate/`、`export_import/`、`auth_file/`、`switch/`、`codebuddy_cli/`、`codebuddy_ide/`、`codebuddy_cn_ide/`、`codebuddy_ide_session{,_sync}/`、`vscode_session{,_sync,_link}/`、`vscode_ext/`、`vscode_cn_inject/`、`jetbrains/`；仅 `variant.rs`、`oauth.rs`、`refresh.rs`、`notifications.rs`、`error_log.rs`、`update.rs`、`linux_keyring.rs`、`session/link` 的少量入口文件保持单文件。
- 宿主：`src-tauri/src/commands/`、`src-tauri/src/tray/`、`src-tauri/src/update_service/`、`crates/wb-shift-server/src/api/{handlers/,router,host_guard,response,state,static_assets}`。
- Companion：`companion-desktop/src/{plugin/,rail_settings/autostart/,rail_position/,session/}`、`companion-core/src/adapters/{workbuddy/,codeg/{events,child}}/`、`companion-core/src/custom/engine/`、`companion-runtime/src/serve.rs`、`companion-core/tests/lifecycle/`。
- 前端：`pages/sessions`、`pages/accounts`、`settings/auto-*`、`screenshot-demo/`、`App.tsx`、`switch-account-dialog/`、`client-switch-account-*`、`app-shell/`、`persisted-preference` 等；主界面 `src/` 已无 >500 行手写文件，`companion-ui/` 只剩 `desktop/rail-controller.ts`（见表）。
- 去重：`is_unauthorized`/`parse_number`（`config/http.rs`）、`count_items`（`config/store.rs`）、`auth_file_path` 转发 shim 删除（统一 `WbVariant::auth_file_path()`）；`preview helper` 共享因触及同步凭据路径、风险大而暂缓（见下）。

### 仍未拆分的 >500 行文件（逐项登记）

行数口径：`wc -l` 的**总行数**（含空行与注释）。核对时请用同一口径；只看非空行会得到更小的
数字（例如 `rail-controller.ts` 非空 671 行 / 总 734 行），容易误判为「文档过时」。

| 文件 | 行数 | 类别 | 理由 | 后续计划 |
| --- | ---: | --- | --- | --- |
| `crates/wb-shift-core/src/modules/limits/scan.rs` | 554 | 来源枚举 | 六个来源的**候选文件收窗**共用同一套判据（按文件 mtime 而非目录名收窗、字节级 BMH 粗筛、`MERGE_WINDOW_MS` 去重），这四件事互为前提：改任一处口径都会让「同一次 429」在另一个来源里重复或消失。第六轮加入「插件宿主多日志根 union + 插件目录过滤」后越过阈值。 | 下一轮把「插件宿主 agent 日志」（`agent_log_roots` / `agent_log_files` / `is_codebuddy_extension_dir` / `walk_agent_logs`）整体抽到 `limits/scan/agent.rs`，公共收窗与去重留在本文件。 |
| `crates/wb-shift-core/src/modules/session/tests/copy.rs` | 3465 | 测试 | 本轮拆分后的测试分组（原 session/tests.rs 6696 行按主题拆分），不属于生产逻辑；再按行数机械切只增加跳转成本。 | 测试文件继续显著膨胀时，按用例主题再拆；当前保留。 |
| `crates/wb-shift-core/src/modules/limits/tests.rs` | 2329 | 测试 | 限额模块测试整体随模块搬迁，测试名与基线一致；第六轮加入「无模型条目按同账号同 resetAt 归并」「多日志根 union」「插件目录过滤」三组用例后超过原登记值。 | 同上，保留；继续膨胀时按「时间戳/归因」「页载荷聚合」「来源枚举」三组拆文件。 |
| `crates/wb-shift-core/src/modules/session/tests/recovery.rs` | 1518 | 测试 | 会话恢复主题测试分组。 | 同上，保留。 |
| `crates/wb-shift-core/src/modules/vscode_cn_inject/tests/inject.rs` | 542 | 测试 | state.vscdb 锁冲突与重试用例（busy_timeout / 失败分类 / 重试循环）共用同一套 `hold_exclusive` + `insert_row` 夹具，拆开会让「同一把库、同一把锁、唯一变量是 attempts」的对照失效。 | 测试继续膨胀时把「锁冲突与重试」用例组拆到 `tests/inject_lock.rs`。 |
| `crates/wb-shift-core/src/modules/diagnostics/redact.rs` | 540 | 脱敏纯函数 + 逐规则单测 | 占位符规则与「看前一个词」的两条上下文规则（`Bearer` 短令牌、含空格路径的后续片段）互相抢匹配，顺序本身就是契约，单测必须与实现同文件才能一眼对上顺序；测试约占 40%。 | 规则集稳定后把测试移到 `diagnostics/redact/tests.rs`（需要把文件改成目录模块）。 |
| `crates/wb-shift-core/src/modules/config/tests.rs` | 1282 | 测试 | 配置模块测试整体随模块搬迁。 | 同上，保留。 |
| `crates/wb-shift-core/src/modules/session/tests/sync.rs` | 1737 | 测试 | 会话同步主题测试分组（含「末条 incomplete 归一后不得永久分歧」的四条收敛用例，以及走真实复制链路 + 组载荷装配（`detail_at`）断言 `summaryStatus` 的三成员组场景，见 issue #3 最后一条）。「三成员组不得被末条归一误判为分歧」这条另带反假绿锚点（成员正文必须真读到：三个成员 `contentState == "ready"` 且 `recordCount > 0`），避免断言在空载荷上假绿。 | 同上，保留。 |
| `crates/wb-shift-core/src/modules/session/tests/exec.rs` | 1390 | 测试 | 会话执行/恢复主题测试分组（本轮加入「同步侧把未来时间戳夹到本次写入时刻」的用例，以及「目标侧既有未来值不得被夹小」的对照用例）。 | 同上，保留。 |
| `crates/wb-shift-core/src/modules/session/tests/run.rs` | 867 | 测试 | 会话编排主题测试分组。 | 同上，保留。 |
| `companion-ui/src/desktop/rail-controller.ts` | 734 | 单一状态所有者 | 悬浮栏的唯一状态所有者（文件头已声明设计意图），拆分会让同一份布局/动画状态跨文件。 | 保持单文件；如继续增长，先抽纯函数（布局计算），状态仍留在 controller。 |
| `crates/companion-core/tests/lifecycle/codex.rs` | 726 | 测试 | Codex 生命周期集成测试分组（原 lifecycle.rs 拆出）。 | 保留；测试文件继续膨胀时按场景再拆。 |
| `crates/wb-shift-core/src/modules/process/macos.rs` | 714 | 平台代码 | macOS 进程枚举/关闭/启动与 app 路径探测集中在单文件，整模块 cfg(macos) 门控；本机 Windows 无法编译验证，先不二次拆。 | macOS CI 覆盖后按「进程控制」「app 路径解析」再拆。 |
| `crates/wb-shift-core/src/modules/session/copy/db_spaces.rs` | 539 | 会话复制 | 目标账号空间匹配、新建与回滚共用同一份路径归一（`space_path_key`）与列探测结果：拆开会让「匹配口径」与「建行口径」分叉，而两者必须对同一目录给出同一结论。 | 下一轮按「查找（`find_target_space_id`/`SpaceLookup`）」与「新建/回滚（`insert_target_space`）」拆成 `copy/space_lookup.rs` 与 `copy/space_create.rs`；拆分前先补两者共用的路径键单测。 |
| `crates/wb-shift-core/src/modules/session/copy/db.rs` | 530 | 会话复制 | 复制写库是**一次事务**：读源行、按目标表列取交集、映射项目归属（`space_id`）、搬账本、写后校验都在同一个事务与同一次列交集结果上完成，拆开就得把这些上下文跨文件传递。本轮加入「活跃时间 = max(源值, 副本正文最新时间戳)」的归一后越过阈值。 | 下一轮把账本搬运（`SessionAux` / `read_session_aux` / `write_session_aux` / `AUX_WRITE_FAILED_PREFIX`）整体抽到 `copy/db_aux.rs`，sessions 行写入、列交集与时间列归一留在本文件。 |
| `crates/wb-shift-core/src/modules/session/recovery.rs` | 503 | 恢复编排 | 复制与同步的中断恢复共用同一套「阶段只前进、已越过只核验不重放」的框架与 `RecoverOutcome` 汇报口径；拆开会让「阶段判定」与「是否需要人工处理」的结论在两份实现里分叉。本轮为复制补写活跃时间多带了一个正文时间戳分支后越过阈值。 | 下一轮按操作类型把 `recover_copy_operation` 与同步恢复拆成 `recovery/copy.rs` + `recovery/sync.rs`，共享的阶段枚举与汇报结构留在本文件。 |
| `crates/wb-shift-core/src/modules/checkin/tests/window.rs` | 702 | 测试 | 签到的时间窗口、到点口径（跨日补签 / 只看今日 / 窗口外限量补签）与周期延迟三组规则互为前提，用例集中放置便于对照口径差异。 | 测试继续膨胀时按「窗口解析」「到点判定」「周期延迟」三组拆文件。 |
| `crates/wb-shift-core/src/modules/variant.rs` | 712 | 单一来源 | 档位（国内/国际）规则与路径的单一来源，刻意集中；拆开会让同一规则出现多个入口。issue #232 后「`domain` 能否判定档位」也在这里收口：`workbuddy.ai` 与 `codebuddy.ai` 的 apex / 子域都算国际版（导入第三方号池时 `domain` 是唯一线索），判据与用例必须同处一个文件才不会被复制成两份口径。 | 保持单文件；新增档位时只在这里扩展。 |
| `crates/wb-shift-core/src/modules/session_link/tests/sync_decision.rs` | 746 | 测试 | 同步判定测试分组（含「只差末条归一签名算一致」的五条纯函数用例）。 | 保留。 |
| `crates/companion-core/tests/custom.rs` | 541 | 测试 | 自定义集成集成测试，整体随上游迁移。 | 保留。 |
| `crates/wb-shift-server/src/api/tests.rs` | 677 | 测试 | api 层边界测试集中在一个文件（SPA 回退 / Host 边界与 `WB_SHIFT_ALLOWED_HOSTS` 白名单 / Origin 口径选择 `origin_policy` / 请求体 DTO 契约 / 切换运行标记 / 清理接口的破坏性默认），共享 `router_request`、`post_json`、`router_status` 三个 helper，按域拆文件会把它们复制到每个子文件。 | 保留；边界层测试再显著膨胀时，先把三个 helper 抽到 `api/tests/support.rs` 再按域拆。 |
| `crates/wb-shift-core/src/modules/session_groups/tests/plugin_liveness.rs` | 823 | 测试 | 插件 / IDE 的存活判定主题测试组（会话目录证据、空工作区分母、`Partial` 判不了时清理入口的 `pruneBlocked`、隐藏集合 ≡ 清理集合、客户端与档位隔离），共用 `tests/editor.rs` 的 `TempStore` 造数夹具；按主题分组而非按行数拆分。 | 继续膨胀时把 `pruneBlocked` 相关用例拆到 `tests/prune_blocked.rs`。 |
| `crates/wb-shift-core/src/modules/session_groups/view.rs` | 1565 | 视图装配 | 列表/详情/卡片载荷共用一份成员快照与判定矩阵；拆开会让「同一份摘要口径」跨文件传递，先保持单文件。列表的存活过滤按客户端分派（WorkBuddy 读库、插件/IDE 走 `liveness`），与清理共用同一份组级汇总。本轮为「组徽标能不能被测试」抽出 `detail_at`（显式注入关联存储根 + 成员路径解析器 `PayloadContext`）：判定、聚合与徽标合成必须仍与生产同一份实现，否则「纯函数测到了、真实载荷没接上」的缺口会重演。跨组批量同步需要的 `behindMemberCount` 也从同一份判定矩阵派生（issue #231：前端据此显示「同步 N 个待同步组」并汇总副本数），不再单独数一遍成员状态。 | 继续增长时把卡片载荷装配（`group_payload`）与共用口径（`member_snapshot`/`summary_text`）按职责再拆；`detail`/`detail_at` 的转发关系保持单文件。 |
| `crates/wb-shift-core/src/modules/vscode_session_sync/tests/sync.rs` | 523 | 测试 | 插件侧同步/预览主题测试分组（含代表成员与存活过滤用例）。 | 保留；测试继续膨胀时按「复制登记」「预览」「同步执行」再拆。 |
| `crates/wb-shift-core/src/modules/session_groups/tests/maintenance.rs` | 1131 | 测试 | 失效组清理与列表过滤的判定/备份/拒绝语义共用一套造数夹具（临时工具存储根、`sessions` 表、插件工作区索引）；补齐插件与 IDE 的清理、索引部分损坏（`Partial`）的拒绝与放行、无失效组时不重写主表、两客户端互不串用等用例后超过阈值，属测试分组而非生产逻辑。 | 继续膨胀时按「WorkBuddy（`sessions` 表）」与「插件 / IDE（工作区索引）」两个主题拆成 `maintenance.rs` + `plugin_prune.rs`。 |
| `crates/wb-shift-core/src/modules/session_groups/tests/editor.rs` | 571 | 测试 | 管理页/客户端作用域/目标兼容性测试与共享夹具 `TempStore`（临时工具存储根、`sessions` 表、插件工作区索引造数）集中在一个文件；夹具被同模块多个 `include!` 的测试共用，按行数拆开只会把同一份造数逻辑复制多份。 | 夹具继续增长时抽到 `tests/support.rs`（与 `vscode_session_sync/tests/fixtures.rs` 同构）。 |
| `scripts/smoke/assert-update-log.py` | 728 | 脚本（断言库） | `--smoke-self-update` 日志的断言入口：`phase` 词表必须与 `src-tauri/src/update_service/smoke.rs` 的 `PHASES` 逐字对照，所以词表、读取、分组、判定、CLI 刻意放同一个文件（拆开会出现第二份词表）；行数的六成是「为什么这样断言」的文档与 `--selftest` 内联的 3 份 JSONL 夹具（正常 / 缺阶段 / 含 error），`--selftest` 必须能在无网络、无 CI 环境的机器上一条命令跑完。 | 词表与断言口径稳定后，把 3 份夹具移到 `scripts/smoke/fixtures/update-log/` 下的 `.jsonl` 文件，正文可降到 500 行以内。 |
| `scripts/smoke/common.py` | 945 | 脚本（结果契约唯一真源） | `artifacts/release-smoke/<form>.json` 的**所有**字段名 / 白名单 / 校验 / 读写都只在这里定义：`common.sh` / `common.ps1` / 5 个平台驱动 / `render-conclusion.py` 全部经它（import 或 CLI）取值。拆成「常量 + 校验 + 读写 + CLI」四份文件会让同一份白名单出现多个入口，而契约分叉的症状是「某平台的格子在 Gate 里静默变 ❓」——正是本文件头注释里记录的两次事故。文档（字段命名口径、两个层级 `upgradeMode` 的口径、`selftest` 的坏数据回归表）也占了不少篇幅，它们是这个契约能被正确使用的原因。 | 只有在契约要加第二个版本号（`schemaVersion=2` 双读）时才拆：届时按「v1 只读适配器 / v2 契约 / 公共工具」三份拆，白名单与 `validate_*` 仍留一份。 |
| `scripts/smoke/preflight.py` | 895 | 脚本（预检） | 「能不能验」的判定全在一处：tag↔版本一致性、Release 是否正式版、资产清单（8 份 `latest*.json` 清单 + 5 个参与自动更新的包及其 `.sig` + 2 个 dmg）、上一版 tag（`git tag --sort=-v:refname`）、`isLatest`、GHCR manifests。六项都是「缺一项就该整条 run 跳过」的同级前置条件，且共用同一份 `--network on/off` 与 gh 探测封装；按检查项拆文件会把这份探测封装复制 6 份，而它的「失败也返回结构化结果、绝不抛异常」纪律正是预检能被 Gate 读懂的前提。 | 接入新的发布物形态（例如再出 arm64 dmg）时，先把「资产清单」抽成 `preflight/assets.py` 的数据表 + 断言函数，其余留在原文件。 |
| `scripts/smoke/render-conclusion.py` | 925 | 脚本（Gate + 矩阵） | 两个阶段的**判定口径必须同一套**：`--gate`（每个验收 job 的最后一步）与 `--matrix`（triage 的结论）共用 `verdict_of` / `STATUS_MARKS` / `aggregate_cells`，矩阵的行列与结论首行又要与 `common.py` 的白名单对照。拆开会让「job 绿了但矩阵红」或反之（D2/D4 两个回归锁就是为此加的）。另外约 1/5 是自测：判定口径、符号表、跨文件版本一致性、坏文件与好文件同批。 | 只有当矩阵要支持「按形态自定义列」时才拆：那时把 `MATRIX_COLUMNS` + `aggregate_cells` + `render_matrix_cell` 抽成 `render/matrix.py`，`--gate` 留在原文件。 |
| `scripts/smoke/classify.py` | 973 | 脚本（A–J 归类） | 归类是「结果 → 处置计划」的**唯一**决策点（不执行任何 gh 命令，因此归类错了也不会有副作用）。A–J 映射、关键词兜底、`issue_title()`（issue 去重的唯一真源，`dispose.py` 也 import 它）、计划项的字段顺序、两类「读不到 vs 契约非法」的路由（DEF-4）都在这里；dispose / issue / render 三个下游都读同一份 `triage-plan.json`。拆开会让「什么算 A 类」出现第二份定义，而 A 类会触发自动重跑。 | 类别表稳定后按「类别判定（A–J 映射 + 推断）/ 计划构造（含 `issueTitle`）/ CLI + 自测」三份拆，`issue_title()` 与 `CATEGORY_ACTIONS` 必须留在同一份。 |
| `scripts/smoke/dispose.py` | 818 | 脚本（处置闸门） | 整条链里**唯一**会改变仓库外部状态的一步，所以四道闸门（单 run ≤3 / 每类每版本 ≤1 / `attempt>=2` 不重跑 / 白名单动作）与「怎么拼 gh 命令」写在同一处：把它们分开就等于让「闸门」与「执行」之间出现可绕过的一层。约 1/4 是自测（优先级、上限、attempt 闸门、dry_run 零调用、rejected、命令形态）。 | 若将来引入新的处置动作（例如 `dispatch-build`），先把白名单 + `build_command` + 闸门抽成 `dispose/policy.py`，CLI 与报告渲染留在原文件。 |
| `crates/wb-shift-core/src/modules/jetbrains/detect.rs` | 791 | 客户端识别 | WorkBuddy / CodeBuddy IDE / CLI / VS Code / JetBrains 五类客户端的安装路径、进程、插件目录与版本探测共用一组平台分支（同一路径要判多个客户端）；拆开会把同一份平台判断复制成多份。第三轮新增 JetBrains 诊断入口、第五轮（R5-19）加入双配置根与 16 产品识别后超过阈值；产品表已抽到 `jetbrains/products.rs`，本文件只剩安装根解析、配置目录映射与进程枚举。 | 下一轮改识别逻辑时按客户端抽 `jetbrains/detect/{workbuddy,codebuddy,jetbrains}.rs`，共用平台分支留在 `mod.rs`。 |
| `crates/wb-shift-core/src/modules/jetbrains/tests/switch.rs` | 809 | 测试 | 切换流程用例（账号数据、IDE 状态、失败回滚、诊断输出）共用一套造数夹具，属测试分组而非生产逻辑。R5-19 的产品表 / 双配置根用例另放 `tests/products.rs`，本文件不再膨胀。 | 继续膨胀时按「正常切换」「失败回滚」「诊断」三组拆文件。 |
| `crates/wb-shift-core/src/modules/session_groups/graph.rs` | 1396 | 批次编排 | 统一批次（复制/同步/登记/回滚）刚越过阈值；计划与实际执行共享同一份批次状态，拆开要跨文件传递可变状态。本轮把组级 N 路识别（`common_base_branches`）的摘要换成与 `decide_sync` 同一套比较口径（`comparable_line_digests`），它必须与两两判定同处一组判据里，否则「组徽标 diverge、两两 behind」会自相矛盾。 | 新增客户端类型时先把批次计划抽到 `session_groups/graph/plan.rs`，执行留在原文件；N 路识别与聚合判定保持同文件（同一份比较口径）。 |
| `crates/wb-shift-core/src/modules/session_groups/tests/graph.rs` | 505 | 测试 | 摘要口径（注入块剥离、包装标签拆解、工具载荷过滤、按项目分组的时间/摘要装配）的回归用例集中在同一份造数夹具上，`TempStore` 与 `view.rs` 的沙箱同源；按行数再拆只会把「同一份正文造数」复制成两份。 | 继续膨胀时把「excerpt 文本清理」的纯函数用例拆到 `tests/excerpt.rs`。 |
| `crates/wb-shift-core/src/modules/session_groups/members.rs` | 569 | 成员操作 | 成员增删与组一致性校验共用同一份成员快照（`member_snapshot`），拆开会让校验与写入读到不同版本。 | 下次改成员语义时把「校验」与「落盘」按职责拆开。 |
| `crates/wb-shift-core/src/modules/session_groups/sync.rs` | 591 | 配对 | 配对算法与失败回滚共用同一事务状态；第三轮加入 JetBrains 一致性守卫后超过阈值。 | 下一轮把「配对判定」抽成纯函数文件，回滚保持原位。 |
| `crates/wb-shift-core/src/modules/session_groups/tests/jetbrains.rs` | 833 | 测试 | JetBrains 组同步/守卫用例，共用 `tests/editor.rs` 的造数夹具。 | 继续膨胀时按「守卫拒绝」「同步执行」拆两组。 |
| `crates/wb-shift-core/src/modules/token_stats/jsonl.rs` | 568 | 流式解码 | JSONL 解码与 per-account 聚合在同一遍流式解析里完成；拆开要么二次读盘，要么跨文件持有游标与累加器。 | 下一轮把 `SourceCollector` 的累加逻辑抽到 `token_stats/collector.rs`，解码留原位。 |
| `crates/wb-shift-core/src/modules/token_stats/projection.rs` | 539 | 投影聚合 | 维度/账号/会话三套累加器共享同一份投影上下文，拆开会让口径分叉（第三轮刚补齐「全局 = 各账号之和」）。 | 保持单文件；新增维度时先抽 `projection/dims.rs`。 |
| `crates/wb-shift-core/src/modules/update.rs` | 872 | 更新清单与状态 | 清单 URL 优先级、semver 比较、检查缓存键与失败退避共享同一组常量与状态；第三轮加入代理/并发/负缓存，第四轮加入 `notes` 截断与 `autoCheck` 配置后超过阈值。 | 下一轮把清单获取与解析抽 `update/manifest.rs`（`fetch_manifest`/`manifest_urls`/`manifest_notes`），状态机与配置留在 `update.rs`。 |
| `src-tauri/src/update_service/mod.rs` | 711 | 更新状态机 | 检查（single-flight + 20s 预算）、阶段流转入口与单测共用同一份进程级状态（快照 / BUSY / 上次检查结果）；测试必须与状态机同文件才能复用 `TEST_LOCK` 串行化，拆开会引入跨文件竞态。第五轮 R5-03 在 `check()` 里加了「发现新版本」系统通知（前台判定 + 版本去重，判定逻辑在 `state.rs`，投递在 `tray_bridge.rs`），四组新单测也落在本文件；发版后验收新增 `mod smoke`（2 行）。0.3.1 为唯一漏锁的 `snapshot_serializes_camel_case_contract` 补上 `lock()`，并在 `TEST_LOCK` 上写明「会写全局状态的函数」完整清单防复发（含 `PACKAGE` 的三个写入口，+20 行）。 | 下一轮把检查编排抽 `update_service/check.rs`，测试随之迁移。 |
| `src/pages/settings/settings-sync-card.tsx` | 850 | 页面卡片 | 设置同步的预览/二次确认/备份还原/未登录账号出口共用同一份 `plan` 与 `config` 状态；拆成多个子组件会把「当前选中备份」「待确认计划」跨文件传递。第四轮补 3 个 P0 与还原入口后超过阈值。 | 下一轮把「备份列表 + 还原对话框」抽到 `settings/settings-backup-section.tsx`，主卡片只保留状态与动作。 |

| `crates/wb-shift-core/src/modules/limits/vscode_ext.rs` | 542 | 探针 | VS Code 插件日志的增量采样（偏移/大小/mtime、强信号、脱敏样本）与四个单测共用同一组信号判定；拆开会让「什么算限额」出现第二份定义。 | 保持单文件；若接入正式 ScanSource，把解析抽到 limits/parse/vscode.rs。 |
| `crates/wb-shift-core/src/modules/process/mod.rs` | 563 | 平台代码 | Windows 关闭路径的「等待集合」与「允许强杀集合」必须同源判定（`close_wait_targets` / `force_kill_targets`），拆开会让两条判据在窗口状态变化时分叉，正是丢未保存内容的入口。 | 下一轮把 `cfg(windows)` 助手与其单测迁到 `process/win_close.rs`，`mod.rs` 只留公共出口。 |
| `crates/wb-shift-core/src/modules/codebuddy_cli/switch.rs` | 510 | 账号与切换 | 关闭 → 切号 → 落盘 → 记录最后使用是一条不可分割的成功链，拆开会让「已在成功后记录」的判据与错误返回点分离。 | 下一轮把进程终止段（含 `force_kill_targets` 用法）拆到 `codebuddy_cli/terminate.rs`。 |
| `crates/wb-shift-core/src/modules/account/tests/display.rs` | 578 | 测试 | 展示对象、本机字段继承与「最近切换」三组用例共用同一套账号夹具；拆开要复制夹具。 | 测试继续膨胀时按「展示字段」「本机字段继承」「最近切换」拆。 |
| `crates/wb-shift-core/src/modules/export_import/tests/import.rs` | 1001 | 测试 | 导入合并的不变量用例（本机字段保留、备份字段采信、旧备份兼容）互为前提，分开后容易只测一个方向；issue #232 的第三方号池兼容用例（camelCase 别名归一化、`{accounts:[]}` 包装、`enc:v1:` 密文）与合并语义共用同一份 `record` / 断言口径，因此同文件分组。QA 复核轮补的边界用例（真实号池形态的字段保真、别名回退的 null/数字/普通对象、顶层标量拒绝、`enc:v2:` 不误判、密文不覆盖本地同 uid 账号、`domain` 决定档位的端到端口径）与同一批断言口径共用 `preview_accounts` / `merge_import_records`，拆开会让「预览说有凭据」与「真的能入库」这对断言分裂。 | 同上，保留；第三方格式再增加（例如别的号池工具的字段命名）时按「本工具格式」与「第三方格式」拆两组。 |
| `src/pages/accounts/ordering.test.ts` | 535 | 测试 | 九种排序维度共用同一组账号样本与「默认口径不变」的对照断言，拆开会让默认排序的回归保护分裂；第六轮补了一条 `lastUsedAt: 0` 的口径用例后超过原登记值。 | 超过 600 行时按「默认排序」「最后使用」「创建时间」拆。 |
| `crates/wb-shift-core/src/modules/settings_sync/apply.rs` | 704 | 同步执行 | 预览/应用/二次确认/备份/还原共用同一份 merge 结果与备份命名（stamp）；拆开会让「预览承诺」与「实际写入」跨文件传递。 | 下一轮把备份列表与还原抽到 settings_sync/backup.rs。 |
| `crates/wb-shift-core/src/modules/settings_sync/tests.rs` | 562 | 测试 | 三个 P0（自动同步写状态、纯 revision 过滤、工作区配置计数）与还原/二次确认用例共用同一套临时目录夹具。 | 保留；继续膨胀时按「预览」「应用」「还原」拆三组。 |
| `crates/wb-shift-core/src/modules/token_stats/tests/ide_roots.rs` | 573 | 测试 | IDE 目录布局（含按客户端拆 source）与副本并集去重用例共用造数函数。 | 保留；膨胀时按「客户端拆分」「副本并集」拆两组。 |
| `crates/wb-shift-core/src/modules/vscode_session/copy.rs` | 571 | 复制 | JetBrains 复制要同时处理 projectKey / bucket / 会话 id 三层路径，且与 VS Code 分支共享同一套索引重写；拆开会让两条路径的校验分叉。 | 下一轮把 JetBrains 路径解析抽到 vscode_session/jetbrains_paths.rs。 |
| `crates/companion-core/src/hub.rs` | 510 | 会话状态机 | 等待/授权/提问三态与 grace 投影共用同一份成员快照；拆开会让「快照里是否出现」与「通知是否发出」判据分叉。 | 下一轮把 `permission-attention` 相关投影抽到 `hub/attention.rs`。 |
| `crates/wb-shift-core/src/modules/automation_sync.rs` | 761 | 定时任务同步 | 只读统计、备份、复制、暂停与设置读写共用同一份 `automations` 列清单与判定函数，拆开会让「跳过的任务」与「复制的列」口径分叉；其中约 300 行为单测。 | 下一轮把 `tests` 段拆到 `automation_sync_tests.rs` 即可回落到 500 行内。 |
| `crates/wb-shift-core/src/modules/refresh.rs` | 2382 | 认证刷新 | 失败分级、标记自愈、单飞与退避共用同一份账号库读写与 `accounts.json` 语义；拆开会让「写标记」与「清标记」读到不同版本。本轮 T01 加入 live 认证文件的 adopt / skip 判定（`decide_live_action`），它必须在 `credentials.lock` 内与抢锁、重读、写回同处一个临界区，与刷新主路径分开就会把「锁外读到的旧新鲜度」放回来。 | 下一轮把 `verify_*` 复核调度抽到 `refresh/verify.rs`；adopt 判定与刷新主路径保持同文件（同一把锁、同一个临界区）。 |
| `crates/wb-shift-core/src/modules/config/configs/bundle.rs` | 597 | 设置包 | 导出/导入四份配置的白名单、整体拒绝语义、导入前备份与「键名 → 文件」映射共用一份 `SETTINGS_BUNDLE_KEYS`；拆开会让白名单与写入路径各持一份，出现「校验过的键」与「写下去的键」不一致。 | 配置项增加时先抽「键表 + 校验」，读写流程留在本文件；条目稳定后可降到 500 行内。 |
| `crates/wb-shift-server/src/api/dto.rs` | 512 | 请求体 DTO | 宿主层所有 `#[serde(rename_all = "camelCase")]` 请求体集中在一处，字段名必须与 Tauri command 参数逐字一致（`check-api-contract.mjs` 逐条比对）；按域拆文件会让「一份 DTO 一个文件」，但契约校验要与命令表整表对照，拆开反而要维护映射。 | 域继续增多时按业务域拆 `dto/{settings,sessions,accounts}.rs`，`mod.rs` 只做重导出。 |
| `crates/wb-shift-core/src/modules/session/sync/preview.rs` | 1054 | 同步预览 | 判定、指纹缓存、预览凭据绑定三者共享同一份内容身份（`ContentStamp`），拆开会让缓存键与判定前提分叉。 | 下一轮把 `PreviewDecision*` 缓存段抽到 `preview/cache.rs`。 |
| `crates/wb-shift-core/src/modules/session_link/content.rs` | 1021 | 正文解析 | 严格校验、容错修复、指纹与摘要必须读同一份规范化后的记录序列，拆开会让「预览的摘要」与「执行的正文」不是同一份。本轮加入「比较口径」`comparable_line_digests`（把复制/同步写目标时的末条归一签名折算成确定性形式），它必须与存储口径（`line_digests`/`total_digest`）在读同一行的同一个循环里算出，两套口径分开维护正是不拆的理由。 | 下一轮把指纹/缓存段抽到 `content/fingerprint.rs`；比较口径随 `normalize_jsonl` 留在本文件。 |
| `crates/wb-shift-core/src/modules/session_link/tests/content.rs` | 566 | 测试 | 正文主题测试分组（校验/修复/指纹/缓存）。 | 测试文件继续膨胀时按用例主题再拆。 |
| `src/pages/credit-stats/data.ts` | 701 | 纯逻辑 | 档位过滤、客户端筛选（明细重算）、账本形状转换三套口径都在这里收敛，页面只调用；拆开会让同一口径出现多份实现。 | 下一轮把账本转换抽到 credit-stats/ledger-view.ts。 |
| `src/pages/credit-stats/index.tsx` | 965 | 页面编排 | 范围/档位/客户端三个筛选维度与导出、渐进刷新共享同一份 stats 快照；拆成子组件要跨文件传递 8 个以上状态。失败动作从 `officialUsage.errors[]` 的档位派生（数据通道：桌面端取数失败也是正常 resolve，`catch` 不执行），文案仍走 `asError()`。 | 下一轮把 header（三个筛选 + 导出）抽到 credit-stats/header.tsx。 |
| `crates/wb-shift-core/src/modules/credits/resources.rs` | 541 | 资源解析与投影 | 个人版资源包与企业配额共用同一套取值与时间戳解析（`first_number` / `first_value` / `parse_timestamp_ms`）以及 `credit_result` 投影：企业配额以单条资源呈现，必须与个人版保持同一份字段口径（如 `expireAt` 纯日期按 23:59:59），拆开会出现第二份口径。为资源包补充可选 `startAt`（`parse_timestamp_at` + `resolve_start_at`，按 `CycleStartTime → DeductionStartTime → CreateTime` 取值，纯日期按本地零点）后越过阈值；该次只做积分明细筛选，不在收尾里搬迁文件。本次新增 `credit_failure_result`（把上游 `failureKind` / `httpStatus` 透传给前端，供错误条给下一步动作），只是同一份投影的失败分支，仍不建议单独搬迁。 | 下次改企业配额时把 `parse_enterprise_usage` / `enterprise_credit_result` / `enterprise_id_from_accounts` / `credit_error_result` 与 `EnterpriseUsage` 抽到 `credits/enterprise.rs`，共用取值与时间戳解析留在本文件。 |

| `crates/wb-shift-core/src/modules/credit_ledger/mod.rs` | 573 | 本地账本 | 建表/去重写入/官方修正回写/日-模型-客户端-账号四组聚合/降级策略共用同一份 schema 与 SQL，拆开会让「账本口径」出现多份定义。 | 下一轮把只读聚合抽到 `credit_ledger/query.rs`，写入与 schema 留在 `mod.rs`。 |

> 该表由 `node scripts/check-structure.mjs` **双向校验**：未登记的 >500 行文件、行数与实际不符、
> 已缩小到 500 行以内但条目未删，都会让 CI 失败。新增文件必须先拆分，或按同一格式登记理由与计划。

### 已知 lint 抑制（类级登记，2026-11）

`#[allow(unused_imports)]` 从 **180 降到 91**（2026-11-16 第一轮按模块清理，已清零 89 处）。

第一轮已清零的模块（每个都通过 `cargo check --all-targets` + 模块单测）：
`auth_file`、`account`、`travel`、`official_usage`、`rotate`、`rate_limit_hook`、
`export_import`、`credit_usage`、`switch`、`vscode_session_link`、
`codebuddy_ide_session`、`codebuddy_ide_session_sync`。

剩余 91 处按成因分三类：

| 类别 | 数量 | 为什么不能直接删 |
| --- | ---: | --- |
| 测试可见性（`use super::*` + `include!`） | 22 | 删 allow 后 lib 目标报未使用，但测试目标靠它取符号 |
| `pub(crate) use` 重导出 | 17 | 同样只被测试通过 `use super::*` 使用 |
| 兄弟模块互引 / 平台 cfg | 52 | `copy ↔ paths`、`status ↔ switch` 这类环，且部分符号只在 macOS / Linux 分支使用 |

- **理由**：模块按职责拆分后，子文件用 glob 复用兄弟模块符号；测试模块再通过
  `use super::*` 复用这些导入。部分符号只在 `cfg(target_os = ...)` 分支使用，
  在 Windows 上不可见，单一开发机无法验证删除后的跨平台影响。
- **影响**：可能掩盖真正的冗余导入（第一轮已清掉 89 处）；不影响运行时代码与产物。
- **后续计划**：按模块继续（下一批：`limits`、`session`、`config`、`vscode_session`、
  `codebuddy_cn_ide`、`codebuddy_ide`、`jetbrains`、`vscode_ext`、`session_backup`、
  `vscode_cn_inject`）。每个模块的验收标准：`cargo check --all-targets` 无 warning、
  该模块单测通过、该模块 allow 归零；清完一批下调 `check-structure.mjs` 的基线，
  归零后删除本条登记。
- **登记位置**：本条 + `scripts/check-structure.mjs` 的 `ALLOW_UNUSED_IMPORTS_BASELINE`
  （棘轮：新增即失败）。

### 下一轮候选（已识别，未在本轮执行）

- 未迁移的 `Json<Value>` handler 见 `scripts/api-contract-baseline.json`：客户端切号 8 个（共用
  `copySessions` / `syncSelections` 嵌套类型）、签到 2 个、只读统计 1 个、通知 1 个、token 刷新 1 个、
  配置保存 5 个（配置 schema 在 core）。迁移顺序建议：客户端切号 → 签到 → 只读查询；每迁完一个就从
  baseline 删掉对应条目（脚本会拒绝过期登记）。
- `preview helper` 共享：`session/sync/preview.rs` 与 `vscode_session_sync/preview.rs` 的 4 个 helper 逐字相同、`live_preview_binding` 仅差 archive 字段；需要允许调整 `session_link::preview` 可见性并清理 `session/mod.rs` 导入面后一次完成，完成后必须跑同步凭据相关测试。
- `session/tests`、`limits/tests`、`config/tests`、`companion-core/tests` 继续膨胀时按用例主题再拆。
- `process/macos.rs` 等平台文件在 macOS CI 稳定后按平台职责再拆。
## 构建与发布入口

| 入口 | 命令 | 产物 |
| --- | --- | --- |
| 前端开发 | `pnpm run dev` | Vite dev server（浏览器预览，配合 WebUI） |
| 桌面开发 | `pnpm run tauri dev`（`beforeDevCommand: pnpm run dev:desktop`） | 桌面 App + 嵌入悬浮窗资源 |
| 前端生产构建 | `pnpm run build` | `dist/`（Tauri 与 WebUI 共用） |
| 桌面打包 | `pnpm run tauri build`（`beforeBuildCommand: pnpm run build:desktop`） | 安装包；签名更新包 |
| WebUI / npm 包 | `cargo build -p wb-shift-server --release` | `workbuddy-shift` 二进制，由 `npm/` 平台分包分发 |
| 公开演示 | `pnpm run build:demo` | GitHub Pages 静态站点（`dist/`） |
| 悬浮窗资源 | `node scripts/build-companion.mjs dev\|build\|demo-dev\|demo-build\|clean` | `public/companion/`、`src-tauri/binaries/` |

Tauri 配置在 [`src-tauri/tauri.conf.json`](../src-tauri/tauri.conf.json)：`frontendDist` 指向
`../dist`，`externalBin` 打包 `companion-runtime`，更新公钥与 release 端点也在那里。

### CI

| workflow | 触发 | 内容 |
| --- | --- | --- |
| `.github/workflows/test.yml` | push / PR / 手动 | 三平台 `cargo test -p wb-shift-core` + `companion-core/runtime` 测试 + Linux/Windows clippy（Linux 含 companion-core/runtime）；macOS lint job（`pnpm install --frozen-lockfile` + `pnpm run build`、`cargo fmt --all`、clippy 含 companion 三 crate、server / desktop / companion-desktop 测试、companion-ui typecheck + build:embed、6 个仓库检查脚本 versions / command-routes / api-contract / rust-locks / structure / companion-contract）；前端 `pnpm run lint` + `pnpm run typecheck` + `pnpm test`；msrv job（`cargo +1.95 check` 四个非 Tauri crate） |
| `.github/workflows/build.yml` | `v*` tag / 手动 | 四平台桌面矩阵 + `npm-linux-arm64` job（arm64 runner 原生编译 CLI，不参与桌面发布）：前端构建、server 二进制、npm 平台分包与主包发布、GitHub Packages scoped 镜像（`publish-github-packages` job，备用渠道）、桌面安装包、更新清单、GitHub Release |
| `.github/workflows/github-packages.yml` | 手动（输入 `version`） | 回填：把 npmjs 上已发布的版本镜像到 GitHub Packages（脚本幂等，已存在则跳过） |
| `.github/workflows/pages.yml` | push `main` / 手动 | `pnpm run build:demo` → GitHub Pages |
| `.github/workflows/release-smoke.yml` | `release` published / 手动（`tag` + `dry_run`） | **发版后验收**，与发版流程解耦（不产出发布物、不改源码 / tag、失败不阻断发布）：`preflight`（tag↔版本一致性 + Release 正式版 + 必需资产 + 上一版 tag + `isLatest` + GHCR manifests，`scripts/smoke/preflight.py`）→ 5 个并行验收 job（Windows 真·应用内升级 / macOS `/Applications` 覆盖 / Linux deb + AppImage / npm 全局升级 / docker run+healthy，结果各写一份同构 JSON）→ `triage`（`classify.py` 归类 A–J → `dispose.py` 次数受限的处置 ≤3 → `render-conclusion.py --matrix` → `update-release-note.sh` 写回 Release 正文标记区块 → `issue.sh` 开 issue）。7 个 job 全部已接线；`triage` 的 `if` 是 `always()`，业务步骤要求 `preflight` 通过 |

发版细节与签名密钥位置见 [DEVELOPMENT.md](DEVELOPMENT.md)。

### 发版后验收的隐藏启动参数（`--smoke-self-update`，**仅 CI 使用**）

`release-smoke.yml` 的 Windows 格要验「0.2.4 的用户点更新 → 下载 → 签名校验 → 安装 → 0.2.5」
这条真实链路，桌面壳没有对应入口，因此 `src-tauri/src/update_service/smoke.rs` 提供了一个
**隐藏参数**，走真实的 `update_service` 链路（`check` → `start_download` → 轮询到
`ReadyToRestart` → `restart`），过程写进 JSON Lines 供 CI 断言。**它不进任何用户文档**
（桌面端没有 `--help`，也不写 `README.md` / `docs/cli.md` / `FAQ.md`），只在本节登记。

- **三重闸门**（任一不满足即在产生任何副作用之前 `return`，默认启动零行为变化）：
  ① `std::env::args()` 中存在**整参相等**的 `--smoke-self-update`（禁止子串匹配，与
  `tray::is_silent_startup` 同口径，`--smoke-self-update-x` 不触发）；② `WB_SHIFT_SMOKE_SELF_UPDATE=1`；
  ③ 非截图演示模式（`WB_SHIFT_SCREENSHOT_DEMO`）。
- **日志**：`WB_SHIFT_SMOKE_LOG` 优先，否则 `<store_dir>/smoke-self-update.jsonl`；JSONL 一行一个
  对象，字段 `ts / seq / phase / version / currentVersion / url / error / detail / pid`，每行 flush。
- **`phase` 词表（固定，新增必须同步 `scripts/smoke/assert-update-log.py` 与本表）**：
  `start` · `check` · `check_ok` · `up_to_date` · `download_start` · `download_progress` ·
  `download_done` · `install_start` · `install_ok` · `relaunch` · `done` · `error` · `abort`。
- **退出码**：`0` 走完 / `10` 已是最新 / `11` 检查失败 / `12` 下载或签名失败 / `13` 安装失败 /
  `14` 环境不可用（日志不可写）/ `20` 平台不支持。CI **不用退出码判通过**（Windows 上 NSIS 会让
  进程以不确定码退出），只用它做失败归类。
- **不写持久化状态**：不读写 `github_config.json` 的开关、不落「已升级」标记、不注册到 autostart
  参数；唯一的落盘是那份追加式日志。重启后的新进程若发现日志已有 `install_ok` / `done` 就跳过
  （否则继承的参数与环境变量会让它重复跑一遍、污染 CI 的序列断言）。


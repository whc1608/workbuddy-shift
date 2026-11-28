# WorkBuddy Shift

> 非官方第三方工具，与腾讯 / WorkBuddy / CodeBuddy 无任何关联。
> 本项目基于 workbuddy-switch（MIT）二次开发，是独立产品线，**不兼容原项目的用户数据与更新通道**。

WorkBuddy、CodeBuddy IDE、CodeBuddy CLI、VS Code CodeBuddy 插件与 JetBrains IDE 插件的账号切换桌面 App（Tauri）。

多账号共享登录态：一键切换各客户端账号，把会话以新 id 复制给其他账号并建立跨账号关联；同时汇总积分到期、官方请求用量、Token 消耗与模型限额。

<p align="center">
  <img src="public/icon-transparent.png" alt="WorkBuddy Shift 图标" width="128" />
</p>

**在线演示**：[打开 GitHub Pages 演示](https://magicapple123.github.io/workbuddy-shift/)（只读演示；账号、积分与请求记录均为虚构数据，业务操作已禁用，另含可交互的会话悬浮栏演示）

## 快速开始

前往 [GitHub Releases](https://github.com/magicapple123/workbuddy-shift/releases/latest) 下载对应平台的安装包：

| 平台 | 安装包 | 安装方式 |
| --- | --- | --- |
| macOS Apple Silicon（M 系列，arm64） | `workbuddy-shift_<版本>_aarch64.dmg` | 打开 DMG，将 `WorkBuddy Shift.app` 拖入「应用程序」 |
| macOS Intel（x86_64） | `workbuddy-shift_<版本>_x86_64.dmg` | 打开 DMG，将 `WorkBuddy Shift.app` 拖入「应用程序」 |
| Windows x64 | `workbuddy-shift_<版本>_x64-setup.exe` | 运行安装程序并按提示完成安装 |
| Linux x64 | `workbuddy-shift_<版本>_amd64.deb` / `workbuddy-shift_<版本>_amd64.AppImage` | Debian/Ubuntu 安装 `.deb`；其他发行版给 AppImage 添加执行权限后直接运行 |

macOS 首次启动若提示无法验证开发者，先在 Finder 中按住 Control 点击应用并选择「打开」，或前往「系统设置 → 隐私与安全性」选择「仍要打开」。仅当安装包来自上述官方 Releases、且系统仍提示「已损坏」时，再执行：

```bash
xattr -rd com.apple.quarantine "/Applications/WorkBuddy Shift.app"
```

应用能启动但切换账号时提示无权限，请参阅下方 [macOS 权限说明](#macos-权限说明)。

另有 npm / webui 与 Docker 两种形态可在浏览器中使用，见文末 [npm / webui 版本](#npm--webui-版本) 与 [Docker 版本](#docker-版本)。

## 功能

| 模块 | 说明 |
| --- | --- |
| 账号管理 | OAuth 扫码登录、备份导入导出、删除账号；可为账号添加备注并选择卡片上显示的字段 |
| 账号切换 | 一键切换 WorkBuddy、CodeBuddy IDE、CodeBuddy CLI、VS Code 插件与 JetBrains 插件账号，切换过程实时显示进度 |
| 会话复制 | 把当前账号勾选的会话以新 id 复制给目标账号：源账号数据不受影响，云端归属目标账号（WorkBuddy / CodeBuddy IDE / VS Code 插件）；历史消耗归来源账号，副本只记录自己的新增 |
| 关联会话 | 复制过的会话自动建立跨账号关联；按客户端集中查看同一会话在各账号中的副本状态，支持增量同步、跨组批量同步（一次把多个待同步组的安全快进副本同步完，分歧组自动跳过并说明原因）、分歧处理与新增 / 解除关联；会话已被删除的失效组不再列出，可一键清理关联关系 |
| 积分到期查询 | 自动查询每个账号的积分剩余量与到期时间；7 天内到期高亮，并按紧迫程度排序、标注「建议优先使用」 |
| 积分统计 | 汇总官方请求用量：总览、近 30 天趋势、账号消耗构成、按模型分类与请求明细；可导出 CSV |
| Token 统计 | 按来源（WorkBuddy / 国际版 / CLI / IDE）查看 Token 总览与趋势、构成占比、活跃热力图、项目与模型 Top 10、会话排行；可导出 CSV |
| 统计加载 | 统计数据缓存在应用进程内，启动后在后台预热；同一次运行中再次打开统计页直接显示上次结果，不必重新等全量扫描，也可随时手动刷新 |
| 存储管理 | 看清工具数据、客户端数据、会话资产包与回收站的体积构成；按分类或单个会话清理（先预览再确认），删除先进回收站、可恢复，也可点「在文件夹中显示」打开所在目录 |
| 模型限额 | 合并本机日志与客户端 hook 信号，显示「哪个账号的哪个模型被限流、官方给出的恢复时刻」；hook 可一键接入 / 卸载 |
| 自动签到 | 在设定的时间段内为允许的账号核验签到状态并补签，可按账号单独关闭 |
| 自动旅行 | 按官方状态自动派发或领取「派猫猫旅行」奖励，切换开关后立即生效 |
| 自动轮换 | 后台把积分最紧迫的账号设为 CodeBuddy CLI 的默认账号；检测到有 CLI 会话在运行时跳过本次切换并提示 |
| 会话悬浮窗 | 桌面版内置悬浮栏，集中显示 Codex / WorkBuddy / CodeBuddy IDE / VS Code 插件 / Codeg 会话的运行中、待确认、已完成状态 |
| 外观与入口 | 浅色 / 深色 / 跟随系统；按客户端开关入口与状态轮询；可隐藏「关联会话」菜单；积分条可选主题蓝或绿 / 橙 / 红状态色（默认主题蓝） |
| 设置同步 | 切换 WorkBuddy 账号且本次切换重启客户端时，把当前账号的界面 / 通用设置（主题、外观资源、权限模式、提示音、语言）同步到目标账号；默认开启，可在设置中关闭；覆盖前自动备份，备份失败则不写入 |
| 定时任务 | 切号时把当前账号可搬运的 WorkBuddy 定时任务复制到目标账号，并把原任务改为暂停（不删除）；含技能、连接器或项目空间引用的任务只提示、不复制；默认关闭，可在设置中开启 |
| 通知与日志 | 应用内提示存档（保留最近 100 条）；前端错误落盘，可在设置中打开日志目录 |
| 开机自启 | 桌面版可设为登录时自动启动 |
| 自动更新 | 从 GitHub Releases 检查新版本，整包更新经签名校验 |
| 权限检测 | macOS「App 管理 / 完全磁盘访问」授权的引导与检测 |

## 支持的工具

| 工具 | 账号切换 | 会话复制 | 自动关闭重开 | 自动轮换 | 悬浮窗监听 |
| --- | :---: | :---: | :---: | :---: | :---: |
| WorkBuddy | ✅ | ✅ | ✅ | — | ✅ |
| CodeBuddy IDE（国内版 / 国际版） | ✅ | ✅ | ✅ | — | ✅ |
| CodeBuddy CLI | ✅ | — | — | ✅ | — |
| VS Code CodeBuddy 插件 | ✅ | ✅ | ✅ | — | ✅ |
| JetBrains IDE 插件（IDE 全家桶 / Android Studio） | ✅ | — | ✅ | — | — |

✅ 表示支持，— 表示不支持。设置 →「支持工具」可按客户端逐个开启 / 关闭入口；关闭后该端入口与状态轮询一并隐藏，不影响账号库与其它端；JetBrains 端默认关闭，可在设置中随时打开。

CodeBuddy CLI 切换时会先关闭正在运行的 CLI，当前会话会中断且不会自动重开；其余各端可在客户端运行时自动完成切换。CodeBuddy IDE 首次使用前需手动打开并登录一次。

### 会话悬浮窗

桌面版内置会话悬浮栏（不再依赖外部组件）：把各 AI Agent 的任务状态集中到桌面，一眼看出谁还在运行、谁需要你确认；悬停查看信息卡，支持跳转时点击即可回到原会话。悬浮栏可拖动调整位置，托盘可随时显示 / 隐藏。

| 监听来源 | 跳转到指定会话 | 点击后的行为 |
| --- | :---: | --- |
| Codex（Desktop / CLI） | ✅ | 打开 Codex Desktop 中的指定任务 |
| WorkBuddy（国内版 / 国际版） | ✅ | 打开对应版本中的指定对话 |
| CodeBuddy IDE（国内版 / 国际版） | — | 有工程路径时打开工程，否则只唤起 CodeBuddy |
| CodeBuddy VS Code 插件 | — | 尝试打开会话所属的 VS Code 工程，无法确定时只唤起 VS Code |
| Codeg | ✅ | 打开 Codeg 中的指定聊天会话 |

各来源都会显示运行中 / 待确认 / 已完成状态；CodeBuddy CLI 与 JetBrains 插件不在监听范围内。开启方式：左下角悬浮窗图标，或设置 → 会话悬浮窗；首次使用在「悬浮窗设置」中完成接入（依赖对应客户端的 Hooks / Webhook），监听来源与外观样式也在那里调整。

演示页里的悬浮栏：一条已完成、一条失败的会话，悬停即可展开信息卡，点击可跳回原窗口（数据为虚构）。

![会话悬浮栏演示：已完成与失败两条会话，悬停展开信息卡](docs/images/companion-demo-rail.png)

更多说明见[会话悬浮窗模块文档](docs/companion.md)。

## 使用

1. **添加与导出账号**：账号页 →「OAuth 扫码登录」「导入备份」；「导出」可将勾选账号备份为 JSON
2. **切换账号与账号信息**：账号卡片 →「切换」，可勾选复制当前会话；「账号信息」可添加备注，并选择卡片上显示账号名 / 手机号 / 备注
3. **查看积分与统计**：账号页自动查询各账号积分到期情况，点「刷新积分」手动更新；侧栏「Token 统计」「积分统计」查看用量明细，两页都可「导出 CSV」
4. **切换各客户端账号**：CodeBuddy CLI、CodeBuddy IDE、VS Code 插件、JetBrains 插件均可在账号卡片一键切换；CodeBuddy IDE 与 VS Code 插件支持在弹窗中勾选复制当前账号的会话
5. **管理关联会话**：侧栏「关联会话」按客户端查看同一会话在各账号中的副本状态，把新增内容增量同步到目标账号、处理内容分歧；「新增关联会话」可把会话复制到新账号并建立关联；会话已被删除的组不再列出，点「清理失效关联组」只移除关联关系，不动账号内的会话内容
6. **清理磁盘占用**：侧栏「存储管理」查看工具数据、客户端数据、会话资产与回收站的体积构成；按分类或单个会话清理（先预览再确认），删除进回收站可恢复，也可点「在文件夹中显示」打开所在目录。客户端运行中时会话类清理会被锁定
7. **自动化**：设置 →「自动签到」「自动旅行」按官方状态补签与领取奖励；「CodeBuddy CLI 自动轮换」按积分紧迫程度自动设置默认账号
8. **限额监听**：设置 →「限额监听」接入 hook（CLI / WorkBuddy 实时上报），IDE 端走本地日志扫描
9. **开关入口与外观**：设置 →「支持工具」按客户端开关入口；「外观」切换主题、选择积分条状态色、隐藏关联会话菜单、设置切换时是否默认勾选归档同步；「设置跨账号同步」在切换 WorkBuddy 账号并重启客户端时把界面 / 通用设置同步到目标账号，默认开启、可在设置中关闭
10. **更新与自启**：应用自动检查公开 GitHub Releases，发现新版本可在左下角直接升级；「启动设置」可设为登录时启动

常见问题（令牌保活多久、为什么反复弹钥匙串、限额标记从哪来、复制后「共消耗」为什么不是两边相加、账号会不会被风控）见 [docs/FAQ.md](docs/FAQ.md)。

## 界面预览

以下截图取自内置演示数据，账号与用量均为虚构。

### 管理 WorkBuddy 与 CodeBuddy 账号

账号卡片集中展示登录状态、积分余额和到期资源，临期积分直接标注在对应卡片内，并按紧迫程度优先排列。

![账号管理页面（数据为虚构）](docs/images/accounts-overview.png)

### 关联会话

复制到其他账号的会话会自动建立关联：按客户端（WorkBuddy / CodeBuddy IDE / CodeBuddy 插件）集中展示同一会话在各账号中的副本状态，可把来源账号的新增内容增量同步到目标账号；出现内容分歧时选择要保留的一份，也可把会话复制到新账号并建立关联。

![关联会话页面](docs/images/session-links.png)

打开任一关联组可查看「会话关联图」：同一会话在各账号中的副本以分支图呈现，内容分歧时标出共同旧版与各自的独立更新，选定要保留的一份即可统一到其他账号。

![关联会话详情：内容分歧时的分支图](docs/images/session-links-branches.png)

### 积分统计

积分统计页展示官方请求用量、每日趋势、账号消耗构成、模型分类与请求明细，数据来源与更新时间会明确显示。

![积分统计页面](docs/images/credit-statistics.png)

### Token 统计

Token 统计页按来源展示 Token 总览与趋势、构成占比、活跃热力图、项目/模型 Top 10 与会话排行。

![Token 统计页面](docs/images/token-statistics.png)

### 存储管理

存储管理页把「工具数据 / 客户端数据 / 会话资产包 / 回收站」四类体积汇总成卡片，可按分类或单个会话清理：先预览要删除的内容再确认，删除先进回收站，可恢复（目标位置已有同名内容时拒绝覆盖），客户端运行中时会话类清理会被锁定。

![存储管理页面](docs/images/storage.png)

### 设置

设置页按四组收纳，宽屏（`xl` 及以上）左侧目录可跳转（也支持 `/settings#settings-startup` 这类深链），顶部搜索框可按节标题、行标题与同义词筛选：

- **通用**：外观（主题、积分条状态色、显示关联会话菜单）、支持工具（WorkBuddy / CodeBuddy IDE / CodeBuddy CLI / VS Code 插件 / JetBrains 插件的入口开关）
- **账号与同步**：切号时同步归档、设置跨账号同步、定时任务（切号时同步 WorkBuddy 定时任务，原任务改为暂停）、权限检测
- **自动化**：自动签到（含派猫猫旅行，后者仅国内版）、CodeBuddy CLI 自动轮换、限额监听
- **桌面与维护**：会话悬浮窗、启动设置（开机静默启动）、通知历史、错误日志、诊断包（预览、导出）、凭据（只读解密、写操作解密、平台探测状态）、未完成的写入（查看与结束）、自动更新、设置包（导出/导入）、关于（版本、工具数据目录、许可证、问题反馈）

自动签到（含保活阈值、惰性刷新）、CodeBuddy CLI 自动轮换与限额监听的参数支持「恢复默认」（默认值来自后端，不在前端硬编码）；派猫猫旅行只有一个开关，没有可调参数。「设置包」可把签到、旅行、轮换、限额四份配置导出成一个 JSON 跨机搬运，导入前自动备份、任一字段不合法则整体拒绝。

![设置页面](docs/images/settings.png)

## macOS 权限说明

切换账号需要写入 WorkBuddy 认证文件，macOS 要求授权「App 管理」（或「完全磁盘访问」）：

1. 首次切换报「无权限」时，点「打开系统设置」
2. 优先在 **App 管理** 里打开 WorkBuddy Shift 开关；若没有，则去 **完全磁盘访问** 把 WorkBuddy Shift 拖进带箭头的框
3. 授权后重启本应用生效；设置页「权限检测」可随时验证

## npm / webui 版本

```bash
npm i -g workbuddy-shift
workbuddy-shift              # 启动本地服务 + 自动打开浏览器
workbuddy-shift status       # 终端查看当前账号
```

> 国内镜像（npmmirror）同步可能延迟；若安装时提示找不到最新版本，可加
> `--registry=https://registry.npmjs.org`，或等镜像同步。

**备用渠道：GitHub Packages**（内容与 npmjs 同号，可在仓库的「Packages」页看到）：

```bash
npm login --registry=https://npm.pkg.github.com   # 需要 GitHub token（至少 read:packages）
npm i -g @magicapple123/workbuddy-shift --registry=https://npm.pkg.github.com
```

> `npm login` 只写入 token，不建立 `@magicapple123` 的 registry 映射，所以安装时要带
> `--registry`（否则 scoped 包会去 npmjs 找、必然 404）。长期使用可把 scope 映射写进
> `~/.npmrc`（`npm config set @magicapple123:registry https://npm.pkg.github.com`），之后不必每次带。

GitHub Packages 对公开包也要求登录 token，所以**主渠道仍是 npmjs**，这条只是 npmjs 不可用时的兜底。

> 维护提示：镜像包首次发布后要确认包可见性（仓库 →「Packages」→ 包 → *Package settings*）；
> 若显示 Private 需改成 Public，否则未登录的安装方即使带上 token 也拿不到包。

界面与桌面 App 一致，功能覆盖上方全部模块，但不提供会话悬浮窗、开机自启与自动更新（桌面版专属；网页版设置页顶部也会写明这三项）。自动签到、派猫猫旅行与自动轮换由服务端后台执行，关掉网页也会继续跑。webui 模式下的 macOS 权限由启动服务的终端进程决定；若终端已授权完全磁盘访问则无需额外操作。

npm 版支持 macOS（Apple Silicon / Intel）、Windows x64、Linux x64 与 Linux ARM64；Linux ARM64 只提供 npm / webui 形态，桌面安装包仍只有 x64。

## Docker 版本

```bash
docker run -d --name workbuddy-shift --restart unless-stopped \
  -p 127.0.0.1:57910:57910 -v "$PWD/data:/data" \
  -e TZ=Asia/Shanghai ghcr.io/magicapple123/workbuddy-shift:latest
```

仓库根也带 `docker-compose.yml`：`docker compose up -d` 即可。默认只发布到宿主机 `127.0.0.1`，
适合 NAS / 家庭服务器长期在线跑自动签到与统计；容器看不到宿主机进程，客户端相关功能需要
额外挂载客户端目录。数据卷、局域网与反向代理、UID/GID、已知限制与安全须知见
[Docker 部署](docs/docker.md)。

> 镜像随 `v*` 发版推送到 GHCR；尚未发布、或想跑当前仓库的改动时，用
> `docker compose up -d --build`（或按 [docs/docker.md](docs/docker.md) 的「本地构建与多架构」自行构建）。

## 开发

```bash
corepack enable pnpm    # 或 npm i -g pnpm；版本由 package.json 的 packageManager 固定
pnpm install            # 根目录一次装完根工程与 companion-ui
pnpm run tauri dev      # 前端（Vite）+ 桌面壳；首次会构建内置悬浮窗资源

pnpm run typecheck      # tsc strict
pnpm run lint           # ESLint，warnings 上限 9（棘轮，新增即失败）
pnpm test               # 前端单元测试（vitest）
pnpm run build          # 生产构建：先清悬浮窗演示产物（build-companion.mjs clean），再 tsc + vite build

# 提交前：Rust 侧（先 pnpm run build 产出 dist/，并在 src-tauri/binaries/ 准备 companion-runtime 占位）
cargo fmt --all -- --check
cargo clippy -p wb-shift-core -p wb-shift-server -p workbuddy-shift-desktop -p companion-core -p companion-runtime -p companion-desktop --all-targets --no-deps -- -D warnings
cargo test -p wb-shift-core
cargo test -p wb-shift-server
# 其余 crate（desktop / companion-*）的测试由 CI 按平台拆分执行，见 docs/DEVELOPMENT.md

# 仓库级契约检查（CI 执行同一批）
node scripts/check-versions.mjs
node scripts/check-command-routes.mjs
node scripts/check-api-contract.mjs
node scripts/check-rust-locks.mjs
node scripts/check-structure.mjs
node scripts/check-companion-contract.mjs
```

CI 的 clippy 按平台与包执行（Linux / Windows job 各跑对应 cfg 的 crate 子集），上面这条 clippy 取自 macOS lint job 的全量命令；完整矩阵见 [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md)。

Windows 也可直接双击 `dev-start.cmd`（沙箱家目录、单实例检测都在脚本里）。**工程约定与 AI/人工协作入口见 [AGENTS.md](AGENTS.md)**；分层与目录职责见 [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md)；构建、打包、图标生成与诊断脚本的说明见 [docs/DEVELOPMENT.md](docs/DEVELOPMENT.md) 与 [scripts/README.md](scripts/README.md)。

## 许可

[MIT](./LICENSE)

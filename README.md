# CodeNeo Lite

CodeNeo Lite 通过一个界面管理 Claude Desktop、Claude Code、Codex 等本机 AI 客户端的模型与 MCP 工具。客户端、模型库、MCP 工具和用量在左侧切换。

## 核心特色功能

在「模型库」选择服务商、填写 API Key、验证并应用，即可为两款桌面客户端分别配置第三方模型，也可以随时切回官方模式。

- **Claude Desktop + Kimi Code / 智谱 GLM Coding Plan**　通过内置预设快速接入兼容 Anthropic Messages 的服务，在 Claude Desktop 的模型菜单中切换模型。
- **Codex Desktop + Kimi Code / 其他兼容服务**　快速接入支持 OpenAI Responses 的服务。使用直连时，可将同一账号下的多个模型加入 Codex 的原生模型菜单。智谱 GLM Coding Plan 的内置预设用于 Claude Desktop；在 Codex 使用时需另行提供兼容 OpenAI Responses 的接口。

## 下载安装

以下为 **0.1.7 预发布版（2026 年 11 月 15 日）**。

- **Windows x64**　[下载 EXE 安装包](https://github.com/QianqianTech/codeneo-lite/releases/download/v0.1.7/CodeNeo-Lite_0.1.7_Windows_x64_Setup.exe)，双击后按简体中文向导安装。安装程序未签名，Windows 可能显示系统提示。
- **macOS 13 或更新版本**（Apple 芯片和 Intel）　[下载通用 DMG（待公证）](https://github.com/QianqianTech/codeneo-lite/releases/download/v0.1.7/CodeNeo-Lite_0.1.7_macOS_universal_notarization-pending.dmg)，打开后将应用拖到「应用程序」。已验证 Developer ID 签名和 Apple 安全时间戳，公证待完成，首次打开可能被系统拦截。
- **Ubuntu x64**　[下载 DEB 安装包](https://github.com/QianqianTech/codeneo-lite/releases/download/v0.1.7/CodeNeo-Lite_0.1.7_Ubuntu_amd64.deb)，已在 Ubuntu 22.04 验证。可在下载目录运行 `sudo apt install ./CodeNeo-Lite_0.1.7_Ubuntu_amd64.deb` 安装。

[发布说明](https://github.com/QianqianTech/codeneo-lite/releases/tag/v0.1.7) · [安装包 SHA-256 校验文件](https://github.com/QianqianTech/codeneo-lite/releases/download/v0.1.7/SHA256SUMS)

各版本的改动见 [更新日志](CHANGELOG.md)。

## 启动客户端

在「客户端」选择应用，再点卡片上的启动按钮。托盘菜单也可以快速打开客户端。

- **Claude Desktop** 使用 ClaudeKeeper 的本机应用启动方式。首次接入网关或切换连接方式时，点击「应用并重启」。已接入网关时可用「应用模式」热切模型。
- **Claude Code** 在系统终端启动。若有模型正在等待切换，退出旧会话，等配置应用后再打开新会话。
- **Codex** 和 **Codex CLI** 有各自的启动入口，共用 Codex 用户配置。使用自定义 `CODEX_HOME` 时，CLI 读取对应目录下的配置。
- **WorkBuddy** 可以打开检测到的本机应用。添加模型后，在 WorkBuddy 的对话中选择它。
- **OpenCode、Qwen Code、CodeBuddy Code、Kimi Code 和 Copilot CLI** 在客户端页可提前配置。显示「未检测到」时，可在详情中查看安装说明。检测到兼容版本后可以从 Lite 或托盘启动。应用模型列表后，原客户端启动时会读取配置。

## 配置模型

在「客户端」选择 Claude Desktop、Claude Code 或 Codex，再找到「模型切换」。可切回官方模式，也可为每个客户端单独选择第三方档案。选中档案后可直接「拉取模型」或「验证供应商」。验证会发送一条简短的模型请求。

Codex 和适合直连的 Claude Desktop 档案可以选择「直连兼容接口」或「CodeNeo Lite网关」。直连由客户端访问服务商，网关由 Lite 在本机转发。使用网关时请保持 Lite 运行。Claude Desktop 使用 Anthropic Messages，Codex 使用 OpenAI Responses。

1. 在「模型库」点击「添加模型」，或在「客户端」选择应用后点击「配置新模型」。已接入的服务商也可直接点击「拉取模型」。
2. 选择服务商，填写一次 API Key。已接入的账号可以直接复用已保存的 Key。
3. 拉取列表，勾选需要的模型并一次保存。模型库显示已保存且有关联 Key 的模型。服务商没有目录接口时，可以在添加流程中选择预置模型或手动填写模型 ID。
4. 按客户端页面的提示应用档案、重开应用或新建会话。从 Claude Desktop 配置时，勾选的多个模型会进入同一个可选模型列表。

| 客户端 | 模型接口 | 配置生效 | MCP 连接方式 |
| --- | --- | --- | --- |
| Claude Desktop | Anthropic Messages | 网关内热切，初次接入或更改连接方式时重开 | 本地命令、FrameLeap 快捷接入 |
| Claude Code | Anthropic Messages | 现有会话退出后自动应用，重新运行 `claude` 后用 `/model` 查看 | Streamable HTTP、SSE、本地命令 |
| Codex 桌面端与 CLI | OpenAI Responses | 网关内热切，直连需重开后从原生列表选择 | Streamable HTTP、本地命令 |
| WorkBuddy | Chat Completions | 在 WorkBuddy 的对话模型菜单中选择已添加的模型 | Streamable HTTP、SSE、本地命令 |
| OpenCode 1.17.18–1.x | Chat Completions、Responses、Anthropic Messages | 运行 `opencode`，使用 `/models` | Streamable HTTP、本地命令 |
| Qwen Code 0.27.3–0.x | Chat Completions、Responses | 运行 `qwen`，使用 `/model` | Streamable HTTP、SSE、本地命令 |
| CodeBuddy Code 2.183.0–2.x | Chat Completions、Responses | 运行 `codebuddy`，使用 `/model` | Streamable HTTP、SSE、本地命令 |
| Kimi Code CLI 2.2.0–2.x | Chat Completions、Responses、Anthropic Messages | 运行 `kimi`，使用 `/model` | Streamable HTTP、SSE、本地命令 |
| GitHub Copilot CLI 1.0.110–1.x | Chat Completions、Responses、Anthropic Messages | 运行 `copilot`，使用 `/model` | Streamable HTTP、SSE、本地命令 |

模型配置写入用户级设置。项目配置、环境变量及启动参数可以覆盖相应的用户默认值。WorkBuddy 需先初始化其本机配置。

### 一个档案使用多个模型

- **Claude Desktop** 在客户端页点击「配置可选模型」，从服务商目录勾选模型，或在「手动添加模型 ID」里每行输入一个 ID。在已选列表中可移除模型或点击「设为默认」，一次保存到当前档案。Claude Desktop 会过滤第三方模型 ID，Kimi、DeepSeek 等档案通过 Lite 网关接入。重开 Claude Desktop 可以刷新它的模型菜单。旧会话若提示接续授权，可刷新 Lite 中的 Desktop 状态后，点击「恢复与撤销 → 接续旧会话」。
- **Claude Code** 在档案中展开「Claude Code 模型映射」。可分别配置 Opus、Sonnet、Haiku、Fable、快速模型、子 Agent 和推理模型。未填写的角色沿用档案的默认模型。
- **Codex** 直连档案会将同一账号下已保存的模型写入原生目录，所选档案作为默认模型。网关模式下，在 Lite 中选择和查看实际转发的模型。
- **新增 CLI** 在客户端页勾选已保存的模型并指定默认项，再点击「应用模型列表」。Kimi Code CLI 可以填写模型上下文上限。Qwen Code 会选用 OpenAI 兼容认证，恢复原配置时还原认证设置。

OpenCode、Qwen Code、CodeBuddy Code、Kimi Code 和 Copilot CLI 通过各自的用户配置读取凭据。这些客户端配置含 API Key。Lite 的模型档案库与配置备份使用加密存储。

### 在网关中热切模型

Claude Desktop、Codex 桌面端和 Codex CLI 已接入 Lite 网关后，在 Lite 的客户端页选任意兼容模型档案，保持连接方式为「CodeNeo Lite网关」，再点「应用模式」。原会话的下一条请求会将续聊上下文发给所选档案的供应商。已开始的流式请求由原模型完成。客户端内已加载的模型名称可能需要重开才会刷新。Codex 的用量统计读取客户端日志，热切前已打开的会话可能继续按原模型名归类。

### Claude Desktop 的思考强度

用新版 Lite 重新应用 Claude Desktop 档案，再重开 Claude Desktop。选择模型后，在输入框旁的模型菜单中点 **Effort**。Lite 按每个模型及其实际使用的 Anthropic Messages 接口判断可用档位。网关优先读取模型目录声明的档位；目录未提供档位时，使用以下已经核对的规则。只有思考开关、固定强度、自动路由或未确认有可调档位的模型不添加 Effort。

| 接入接口与模型 | 可区分的档位 | Claude Desktop 菜单中的对应关系 |
| --- | --- | --- |
| [百炼 Token Plan Qwen4 Max Preview](https://help.aliyun.com/zh/model-studio/qwen4-max-preview)、[百炼 Qwen3.8 Max / Flash](https://help.aliyun.com/zh/model-studio/anthropic-api-messages) | Low、Medium、Extra | High 和 Extra 都进入 `xhigh`；最高档为 Extra |
| [百炼 GLM-5.3](https://help.aliyun.com/zh/model-studio/anthropic-api-messages)、[智谱 GLM-5.3 / Flash](https://docs.bigmodel.cn/cn/coding-plan/latest-model)、[Kimi Code K3.1 Pro / K3 / K2.8 Preview](https://www.kimi.com/code/docs/third-party-tools/claude-code.html)、[DeepSeek 官方 Flash / V4 Pro](https://api-docs.deepseek.com/guides/thinking_mode/) | Low、High、Max | Medium 归到 High |
| [百炼 GLM-5.2、DeepSeek V4 Pro](https://help.aliyun.com/zh/model-studio/anthropic-api-messages) | High、Max | Low、Medium 归到 High |
| [百炼 kimi-k3.1-pro](https://help.aliyun.com/zh/model-studio/anthropic-api-messages) | Low、Medium、High、Max | 四档分别转发 |
| [Claude 模型](https://platform.claude.com/docs/en/build-with-claude/effort) | 按型号 | 使用 Claude Desktop 的模型档位 |
| [OpenRouter](https://openrouter.ai/docs/guides/best-practices/reasoning-tokens#discovering-per-model-reasoning-options)、其他 Anthropic 兼容接口 | 按模型目录声明 | 目录中支持的档位按原值转发；只有一个可用档时不显示选择 |

Claude Desktop 的[配置](https://claude.com/docs/third-party/claude-desktop/configuration#inferencemodels)只能限制最高档。对于目录中档位不连续的模型，Lite 会将菜单里的额外选项转成最接近的可用档位。只调节思考预算的模型不视为有离散档位。

### 其他客户端的思考强度

重新应用模型档案或模型列表，启动新会话后使用以下入口。Lite 依据供应商目录和实际接口确认模型档位。固定强度、只有思考开关、自动路由和未声明可调档位的模型沿用客户端原设置。

| 客户端 | 选择入口 |
| --- | --- |
| Claude Code | 在 Lite 的「新会话思考强度」选档后点「在终端启动」。能由原生滑块完整表达的模型也可用 `/effort`。 |
| Codex 桌面端与 CLI | 桌面端模型选择器或 CLI 的 `/model`。网关热切后重开客户端可刷新档位菜单。 |
| OpenCode | 选好模型后切换模型变体，命令行可用 `--variant`。 |
| Qwen Code | 支持模型档位声明的版本可用 `/effort`。 |
| WorkBuddy、CodeBuddy Code | 选好模型后在客户端调整思考强度。 |
| Kimi Code CLI | 在客户端调整 Thinking 强度。 |
| GitHub Copilot CLI | 在可用的模型选择器中调整，或用 `--reasoning-effort`。 |

## 配置 MCP

1. 打开「MCP 工具」，选择目标客户端。展开「配置文件」可查看 Lite 会写到哪里。Claude Desktop 会区分官方模式与第三方模型模式。
2. 从「热门 MCP」选择服务并点「快速配置」。Lite 会选用目标客户端支持的连接方式，有需要时只提示填写 Key、选择目录或在客户端完成登录。Context7、DeepWiki、GitHub、Notion、Playwright 和 Filesystem 已提供预设。
3. 接入其他服务时，点「粘贴配置」导入单个服务的 JSON、JSONC 或 Codex TOML，检查预填内容后保存；也可以点「添加 MCP」手动填写。Codex、Qwen 等客户端支持的工作目录、超时和工具范围可在表单的高级设置中修改。

Claude Code、Codex、Qwen Code、CodeBuddy Code 和 Kimi Code 可以选择项目共享配置；Claude Code 还支持只在本机当前项目生效的配置。快捷配置在项目共享范围使用环境变量引用 Key。

「已配置」可以编辑、验证、移除服务或撤销当前配置范围的最近一次变更。保存只写入配置；主动点「验证连接」时，Lite 会发起 MCP 握手并尝试读取工具列表。OAuth 登录或依赖客户端会话的连接，请在目标客户端中验证。

在 Lite 管理的 Claude Desktop 第三方模式下，可在「已配置 → 本地 MCP 服务 → 工具权限」读取并勾选具体工具，再保存为「始终允许」。规则跟随第三方档案，切换其中的模型无需重复设置。等当前任务结束后，完全退出并重新打开 Claude Desktop。需要收回 Lite 写入的授权或修改服务连接时，可先点「清空授权」并保存。

此入口适用于 Playwright、FrameLeap 等使用 `command`、`args`、`env` 的本地 MCP。Lite 使用 Claude Desktop 的[档案工具权限](https://claude.com/docs/third-party/claude-desktop/configuration#tool-permissions-for-managed-mcp-servers)，只允许选中的工具，不开启 Cowork 的全局 Auto 模式。Claude 自己保存的授权仍在原客户端管理。

Playwright 的 MCP 服务及 npm 依赖已随 Lite 打包，快速配置会直接使用内置服务，不再通过 `npx` 下载。验证旧版 Playwright 配置时，Lite 会先将已知的 `npx` 启动方式迁移到内置入口，并保留启动参数、环境变量和高级设置。本机仍需 Node.js 18+ 和可用的浏览器。

Claude Code、Qwen Code、CodeBuddy Code 和 Copilot CLI 的启停入口在原客户端。WorkBuddy 添加工具后需要在客户端中信任该连接器。

FrameLeap 位于「MCP 工具 → 热门 MCP」首位，展开后会选中 Claude Desktop。在臻悦「设置 → 接入 AI 助手」复制工作区 MCP Key，再点击「验证并接入」。

CodeNeo Lite 使用 [MIT 许可证](LICENSE)。欢迎为 [MCP 目录](mcp/catalog.json) 提交自己的服务适配，详见 [贡献指南](CONTRIBUTING.md)。

## 用量与参考费用

「用量」从 Claude Desktop、Claude Code、Codex 和已检测到的新增 CLI 的本机记录统计输入、输出及缓存 Token。可按客户端查看最近 24 小时、7 天或 30 天的数据。未能读完整的客户端会在总量旁显示「部分统计」。WorkBuddy 的本机用量来源待确认。新增 CLI 的参考费用需要明确的服务商及匹配的公开单价。

直连和经 CodeNeo Lite 网关的调用都按客户端记录合计。网关转发服务商返回的用量字段；没有写入客户端本机日志的请求不会显示在用量页。

费用按模型厂商公开 API 价估算，统一显示整数人民币，有上下限时取中点。美元单价按 2026-11-12 中间价 1 美元兑 6.7174 元人民币折算。DeepSeek 使用公开峰谷价，百炼使用华北 2（北京）公开价。明细提供价格来源、价格核对日期和汇率来源。订阅、套餐、代理、折扣及工具费用请以实际账单为准。

价格表覆盖预置模型，并统一处理已核对的模型别名、供应商前缀和日期后缀。[GLM-5.3 系列](https://docs.bigmodel.cn/cn/guide/start/pricing)按智谱人民币公开价估算；[Qwen4 Max Preview](https://help.aliyun.com/zh/model-studio/qwen4-max-preview)目前仅支持 Token Plan，`qwen4-max-preview` 与账单别名 `qwen4.0-max-preview` 均显示「套餐计费」及官方说明，不虚构按量单价。自动路由或套餐别名未提供实际模型时，显示「实际模型待确认」。未收录的新模型显示「价格待补充」，其 Token 仍纳入总量。

存在未折算记录时，金额显示「已估算小计」及已覆盖的 Token 比例。同一模型部分请求缺少计价依据时，也会保留原因和未折算用量。价格来源与核对日期由后端统一提供，价格表随应用版本更新，不实时抓取官网。开发时运行 `cargo test --locked -p codeneo-lite usage`，其中的覆盖检查要求每个预置模型及角色映射具有计价规则或明确的套餐／动态路由说明；新增模型不能直接落入未知价格。

## 开发验证

在项目根目录运行以下命令。需要 Rust 1.96、Node.js 20 或更新版本，以及 Tauri 平台依赖。

```sh
npm ci
npm run dev
```

```sh
cargo test --locked --workspace
npx playwright install chromium
npm run test:ui
```

若本机已安装 Codex CLI，可用 `CODENEO_TEST_CODEX="$(command -v codex)" cargo test -p codeneo-lite direct_catalog_shows_same_account_models_and_restores_external_catalog` 核对原生模型目录，也可以将测试名换成 `native_codex_continues_a_thread_after_gateway_switch` 并加上 `-- --ignored`，验证同一会话切换上游后的续聊。测试使用临时配置和模拟供应商。

用 `npm run build` 构建当前平台的安装包。应用沿用原有 Lite 的本机数据目录，可继续读取已保存的档案。若 Codex 或 FrameLeap 的连接引用了旧版可执行文件路径，请在新应用里重新应用对应模型或更新 MCP 连接。

macOS 交付前对最终 `.app` 运行 `npm run verify:macos -- /path/to/codeneo-lite.app`，并在 DMG 挂载后再次验证盘内的应用。验证会核对源码、Info.plist 和可执行文件的版本、双架构、代码签名及实际计价逻辑。不要用 `cargo build` 生成的开发可执行文件代替更新 `.app`；用量页的「计价说明」会显示实际运行版本。

`crates/` 包含来自 ClaudeKeeper 的模型配置、目录请求和诊断模块。`src-tauri/src/gateway.rs` 管理 CodeNeo Lite网关的本机监听与启动恢复。

开发运行可通过 `CODENEO_LITE_DATA_DIR` 指定独立的 Lite 档案目录。

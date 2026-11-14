# CodeNeo Lite

CodeNeo Lite 通过一个界面管理 Claude Desktop、Claude Code、Codex 等本机 AI 客户端的模型与 MCP 工具。客户端、模型库、MCP 工具和用量在左侧切换。

## 下载安装

- **Windows x64**　[下载 `0.1.1` 安装程序（.exe）](https://github.com/QianqianTech/codeneo-lite/releases/download/v0.1.1/CodeNeo-Lite_0.1.1_Windows_x64_Setup.exe)，双击按提示安装。
- **macOS 13 或更新版本**（Apple 芯片和 Intel）　[下载 `0.1.1` 通用版（.dmg）](https://github.com/QianqianTech/codeneo-lite/releases/download/v0.1.1/CodeNeo-Lite_0.1.1_macOS_universal.dmg)，打开后将应用拖到「应用程序」。

这是公开预览版。Windows 安装程序未签名，macOS 版已签名，Apple 公证尚未完成。首次打开可能遇到系统提示。 [发布说明与 SHA-256 校验](https://github.com/QianqianTech/codeneo-lite/releases/tag/v0.1.1)

## 启动客户端

在「客户端」选择应用，再点卡片上的启动按钮。托盘菜单也可以快速打开客户端。

- **Claude Desktop** 使用 ClaudeKeeper 的本机应用启动方式。模式或模型列表需要重载时，点击「应用并重启」，让当前窗口退出后读取新配置。
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
| Claude Desktop | Anthropic Messages | 完全退出并重开 Claude Desktop | 本地命令、FrameLeap 快捷接入 |
| Claude Code | Anthropic Messages | 现有会话退出后自动应用，重新运行 `claude` 后用 `/model` 查看 | Streamable HTTP、SSE、本地命令 |
| Codex 桌面端与 CLI | OpenAI Responses | 重开应用或运行 `codex`，从原生模型列表选择 | Streamable HTTP、本地命令 |
| WorkBuddy | Chat Completions | 在 WorkBuddy 的对话模型菜单中选择已添加的模型 | Streamable HTTP、SSE、本地命令 |
| OpenCode 1.17.18–1.x | Chat Completions、Responses、Anthropic Messages | 运行 `opencode`，使用 `/models` | Streamable HTTP、本地命令 |
| Qwen Code 0.27.3–0.x | Chat Completions、Responses | 运行 `qwen`，使用 `/model` | Streamable HTTP、SSE、本地命令 |
| CodeBuddy Code 2.183.0–2.x | Chat Completions、Responses | 运行 `codebuddy`，使用 `/model` | Streamable HTTP、SSE、本地命令 |
| Kimi Code CLI 2.2.0–2.x | Chat Completions、Responses、Anthropic Messages | 运行 `kimi`，使用 `/model` | Streamable HTTP、SSE、本地命令 |
| GitHub Copilot CLI 1.0.110–1.x | Chat Completions、Responses、Anthropic Messages | 运行 `copilot`，使用 `/model` | Streamable HTTP、SSE、本地命令 |

模型配置写入用户级设置。项目配置、环境变量及启动参数可以覆盖相应的用户默认值。WorkBuddy 需先初始化其本机配置。

### 一个档案使用多个模型

- **Claude Desktop** 在档案中展开「Claude Desktop 可选模型」。从目录加入或每行填写一个同一接入账号的模型 ID，首行是默认模型。Claude Desktop 会过滤第三方模型 ID，Kimi、DeepSeek 等档案通过 Lite 网关接入。应用后重新打开 Claude Desktop，在模型菜单中选择。应用新供应商档案后，可回原会话继续，历史消息会随请求发给当前供应商。旧会话若提示接续授权，可刷新 Lite 中的 Desktop 状态后，点击「恢复与撤销 → 接续旧会话」。
- **Claude Code** 在档案中展开「Claude Code 模型映射」。可分别配置 Opus、Sonnet、Haiku、Fable、快速模型、子 Agent 和推理模型。未填写的角色沿用档案的默认模型。
- **Codex** 直连档案会将同一账号下已保存的模型写入原生目录，所选档案作为默认模型。Codex 每次启动使用当前配置的一个服务商。Lite 网关使用当前档案对应的模型。
- **新增 CLI** 在客户端页勾选已保存的模型并指定默认项，再点击「应用模型列表」。Kimi Code CLI 可以填写模型上下文上限。Qwen Code 会选用 OpenAI 兼容认证，恢复原配置时还原认证设置。

OpenCode、Qwen Code、CodeBuddy Code、Kimi Code 和 Copilot CLI 通过各自的用户配置读取凭据。这些客户端配置含 API Key。Lite 的模型档案库与配置备份使用加密存储。

## 配置 MCP

1. 打开「MCP 工具」，选择目标客户端。展开「配置文件」可查看 Lite 会写到哪里。Claude Desktop 会区分官方模式与第三方模型模式。
2. 从「热门 MCP」选择服务并点「快速配置」。Lite 会选用目标客户端支持的连接方式，有需要时只提示填写 Key、选择目录或在客户端完成登录。Context7、DeepWiki、GitHub、Notion、Playwright 和 Filesystem 已提供预设。
3. 接入其他服务时，点「粘贴配置」导入单个服务的 JSON、JSONC 或 Codex TOML，检查预填内容后保存；也可以点「添加 MCP」手动填写。Codex、Qwen 等客户端支持的工作目录、超时和工具范围可在表单的高级设置中修改。

Claude Code、Codex、Qwen Code、CodeBuddy Code 和 Kimi Code 可以选择项目共享配置；Claude Code 还支持只在本机当前项目生效的配置。快捷配置在项目共享范围使用环境变量引用 Key。客户端的信任和授权仍在客户端内处理。

「已配置」可以编辑、验证、移除服务或撤销当前配置范围的最近一次变更。保存只写入配置；主动点「验证连接」时，Lite 会发起 MCP 握手并尝试读取工具列表。OAuth 登录或依赖客户端会话的连接，请在目标客户端中验证。

Claude Code、Qwen Code、CodeBuddy Code 和 Copilot CLI 的启停入口在原客户端。WorkBuddy 添加工具后需要在客户端中信任该连接器。

FrameLeap 位于「MCP 工具 → 热门 MCP」首位，展开后会选中 Claude Desktop。在臻悦「设置 → 接入 AI 助手」复制工作区 MCP Key，再点击「验证并接入」。

CodeNeo Lite 使用 [MIT 许可证](LICENSE)。欢迎为 [MCP 目录](mcp/catalog.json) 提交自己的服务适配，详见 [贡献指南](CONTRIBUTING.md)。

## 用量与参考费用

「用量」从 Claude Desktop、Claude Code、Codex 和已检测到的新增 CLI 的本机记录统计输入、输出及缓存 Token。可按客户端查看最近 24 小时、7 天或 30 天的数据。WorkBuddy 的本机用量来源待确认。新增 CLI 的参考费用需要明确的服务商及匹配的公开单价。

费用按模型厂商公开 API 价估算，统一显示整数人民币，有上下限时取中点。美元单价按 2026-11-12 中间价 1 美元兑 6.7174 元人民币折算。DeepSeek 使用公开峰谷价，百炼使用华北 2（北京）公开价。明细提供价格来源、价格核对日期和汇率来源。订阅、套餐、代理、折扣及工具费用请以实际账单为准。

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

若本机已安装 Codex CLI，可用 `CODENEO_TEST_CODEX="$(command -v codex)" cargo test -p codeneo-lite direct_catalog_shows_same_account_models_and_restores_external_catalog` 核对原生模型目录，也可以将测试名换成 `native_codex_completes_a_turn_through_the_gateway` 并加上 `-- --ignored`，验证本机网关的流式对话。测试使用临时配置和模拟供应商。

用 `npm run build` 构建当前平台的安装包。应用沿用原有 Lite 的本机数据目录，可继续读取已保存的档案。若 Codex 或 FrameLeap 的连接引用了旧版可执行文件路径，请在新应用里重新应用对应模型或更新 MCP 连接。

`crates/` 包含来自 ClaudeKeeper 的模型配置、目录请求和诊断模块。`src-tauri/src/gateway.rs` 管理 CodeNeo Lite网关的本机监听与启动恢复。

开发运行可通过 `CODENEO_LITE_DATA_DIR` 指定独立的 Lite 档案目录。

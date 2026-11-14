# CodeNeo Lite 热门 Agent 接入任务清单

客户端页、模型库目标、MCP 和托盘按检测到的兼容版本显示新增 CLI。应用模型后，从原客户端启动并检查其模型选择器。 [Lite 当前功能](../README.md)

## 执行结果

| 客户端 | 已核对的版本 | 本机验证 |
| --- | --- | --- |
| OpenCode CLI | 1.19.5 | 两个模型完成对话，加载两个 MCP 并实际调用其中一个，读取 SQLite Token。 |
| Qwen Code | 0.27.3 | 两个模型完成对话，读取两个 MCP 和本机 `/stats` 记录。 |
| 腾讯 CodeBuddy Code CLI | 2.183.0 | 两个模型完成对话，读取两个 MCP 和本机会话 Token，核对用户级默认模型。 |
| Kimi Code CLI | 2.2.0 | 配置通过原生 `doctor`，两个模型完成对话，读取两个 MCP 与本机用量事件。 |
| GitHub Copilot CLI | 1.0.110 | 两个模型完成对话，读取两个 MCP 和本机用量事件，核对提供方与模型的完整启动参数。 |

- OpenCode 2.2.7 能完成模型对话，用户级 MCP 在本机对话中未生效，兼容检测暂时跳过 2.x。 [OpenCode 全局 MCP 上游记录](https://github.com/anomalyco/opencode/issues/55811)
- Codex 直连档案写入用户级 `config.toml` 和原生模型目录。同一账号下已保存的模型在 `/model` 中可选。 [Codex 配置](https://learn.chatgpt.com/docs/config-file/config-reference)
- GitHub Copilot CLI 从用户级 `providers.json` 读取 BYOK 模型，从 `settings.json` 读取默认项。 [Copilot 模型配置](https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/add-llm-models)
- 2026-11-14 用 Lite 写入临时用户配置，在 macOS 直接启动表中五款 CLI。默认模型与第二个模型均完成本机模拟对话。Codex CLI 0.163.0 的原生模型目录和两个 Responses 模型也已验证。

## 通用能力

- [x] **AG-01 统一 Agent 能力定义。** 将现有四款客户端的枚举和分派整理为可注册的适配器。每款登记稳定 ID、安装和版本探测、支持的模型协议、MCP 连接方式、启动入口、用量来源和配置生效方式。客户端页和托盘共用这份定义，显示未安装、需初始化和已配置状态。沿用 ClaudeKeeper 的本机应用与终端启动方式。 [客户端](../src-tauri/src/clients.rs) · [启动](../src-tauri/src/launch.rs) · [当前界面](../ui/app.js)
- [x] **AG-02 复用模型库与多模型流程。** 按目标 Agent 支持的协议筛选账号和模型，复用「加载模型列表 → 测试连接 → 保存 → 应用」。每个 Agent 独立保存可用模型、默认模型和可选角色映射。目录接口不可用时保留手填模型 ID，并展示原客户端如何切换与验证生效。 [模型与账号](../src-tauri/src/models.rs) · [模型界面](../ui/models.js)
- [x] **AG-03 扩展多 MCP 与安全写入。** 每个 Agent 支持独立添加、修改和移除多个服务，按能力开放启停及本地命令、Streamable HTTP、SSE。写入用户级配置，提示项目设置的优先级。扩展现有 JSON/TOML 事务以覆盖需要的 JSONC，保留注释、其他配置和未管理条目。沿用密钥存储、原子写入、备份和撤销，验证并发编辑。 [MCP](../src-tauri/src/mcp.rs) · [配置事务](../src-tauri/src/transaction.rs)
- [x] **AG-04 扩展 Token 与人民币参考费用。** 从原生结构化记录、导出或只读命令提取用量，按 Agent、模型和时间查看。核对输入、输出、缓存读写的含义、时区及去重规则。价格匹配考虑服务商、模型、地区及适用阶梯，优先用官方人民币价，其他币种记录汇率与日期后折算。界面区分暂无记录、来源不开放和缺少报价，展示用量覆盖范围、价格出处和估算所用的计价方式。WorkBuddy 的本机用量来源待确认。 [用量](../src-tauri/src/usage.rs) · [估价](../src-tauri/src/usage_prices.rs)

## CLI 接入

- [x] **AG-05 OpenCode 1.x CLI。** 注册多模型、设置默认模型、管理多个本地及远程 MCP，通过本机会话数据库读取 Token。 [配置](https://opencode.ai/docs/config) · [MCP](https://opencode.ai/docs/mcp-servers)
- [x] **AG-06 Qwen Code。** 探测并启动 `qwen`，在 `~/.qwen/settings.json` 接入模型列表及默认模型，优先验证 Chat Completions，核对 `/model` 的切换结果。复用本地命令、HTTP、SSE 多 MCP 管理。从原生统计或结构化导出接入按模型与认证方式的 Token。来源按日汇总时显示其统计粒度。 [模型配置](https://qwenlm.github.io/qwen-code-docs/en/users/configuration/model-providers/) · [MCP](https://github.com/QwenLM/qwen-code/blob/main/docs/users/features/mcp.md) · [原生统计](https://qwenlm.github.io/qwen-code-docs/en/blog/updates/weekly-update-2026-06-25/)
- [x] **AG-07 腾讯 CodeBuddy Code CLI。** 新增独立客户端入口，探测并启动 `codebuddy`。复用 WorkBuddy 的模型编辑与校验逻辑，按 CodeBuddy 的 `models.json`、`availableModels` 和项目覆盖规则注册多模型，在 `/model` 或 `--model` 验证。识别其用户级 MCP 文件优先级，保留已有项目配置。按本机会话请求统计 Token，在用量页显示来源。原客户端可用 `/cost` 核对。 [模型配置](https://www.codebuddy.cn/docs/cli/models) · [MCP](https://www.codebuddy.cn/docs/cli/mcp) · [用量](https://www.codebuddy.cn/docs/cli/costs) · [结构化输出](https://www.codebuddy.cn/docs/cli/common-workflows)

- [x] **AG-08 Kimi Code CLI。** 检测当前 CLI 及 `KIMI_CODE_HOME`，在原生 TOML 配置中写入提供方、凭据、模型和默认项。接入 `mcp.json` 的多个服务。用原生 `/usage` 校验主会话与子 Agent 的统计口径，从本机 `usage.record` 事件读取 Token。 [配置](https://www.kimi.com/code/docs/kimi-code-cli/configuration/config-files) · [MCP](https://www.kimi.com/code/docs/kimi-code-cli/customization/mcp.html) · [用量命令](https://www.kimi.com/code/docs/kimi-code-cli/reference/slash-commands.html)
- [x] **AG-09 GitHub Copilot CLI。** 启动 `copilot`，按检测到的版本验证自带 API Key 的多模型配置及模型选择器，先确认 `providers.json` 的凭据注入方式和覆盖规则。接入多个 MCP 服务，从已验证的 `session.usage_record` 读取 Token，分别显示 GitHub 套餐和 BYOK。 [自定义模型](https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/add-llm-models) · [MCP](https://docs.github.com/en/copilot/how-tos/copilot-cli/customize-copilot/add-mcp-servers) · [配置目录](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference)
## 验收

- [ ] **AG-16 客户端完整流程。** 在 Lite 加载、测试并应用至少两个模型。从系统正常启动原客户端，确认默认模型和可选模型，完成一次真实供应商对话。新增两个 MCP 服务，验证客户端读取及至少一次工具调用，检查更新或移除一个服务后其余服务仍可用。
- [ ] **AG-17 用量与回归。** 用测试会话和脱敏样本覆盖输入输出、缓存和子 Agent 的用量，核对人民币估价和无报价状态。回归 Claude Desktop、Claude Code、Codex、WorkBuddy 的模型、MCP、启动和现有用量行为。使用客户端官方支持平台上的开发运行验证，记录实际版本、测试范围和缺项。 [开发验证命令](../README.md#开发验证)

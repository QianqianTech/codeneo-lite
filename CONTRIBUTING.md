# 为 CodeNeo Lite 贡献 MCP 适配

用户可以在「MCP 工具 → 粘贴配置」接入自己或团队的服务。想让其他人也能在「热门 MCP」里快速配置，可以 Fork 本项目，在 [mcp/catalog.json](mcp/catalog.json) 里添加一条记录并提交 PR。为现有客户端接入新服务只需改目录，无需改 Rust 或前端。

例如，下面是一条无需登录的远程服务。请把示例地址换成真实的服务和文档：

```json
{
  "id": "example-docs",
  "name": "Example Docs",
  "category": "开发文档",
  "description": "按关键词查询产品文档。",
  "source": "https://example.com/docs/mcp",
  "variants": [
    { "transport": "http", "endpoint": "https://example.com/mcp" }
  ]
}
```

[目录 Schema](mcp/catalog.schema.json) 提供编辑器补全。每条记录使用固定的英文小写 ID，并链接到服务维护者的配置文档。描述应说明能做什么。

- `variants` 按优先顺序排列。Lite 为目标客户端选择第一个可用的连接。一般先放 `http` 直连，再放 `stdio` 备选；旧 SSE 服务用 `sse`。如果某种方式只在部分客户端可用，用 `clients` 限定。
- `stdio` 的 `endpoint` 是可执行文件名，`args` 每项为一个参数。用 `requires` 标出 Node.js、Docker 等依赖。Lite 保存配置时会检查程序是否存在；项目共享配置保留命令名，方便在不同机器上使用。保存不会下载依赖或运行命令。
- `options` 用于非敏感的静态请求头或环境变量。需要用户填写凭据时，用 `input` 指定字段，例如 `{"target":"header","key":"Authorization","prefix":"Bearer ","label":"服务 API Key","required":true}`。本地服务可以用 `target: "env"`；需要选择目录时用 `target: "directory"`，所选目录会作为最后一个启动参数传入。
- OAuth 需要由客户端或本地启动器完成。用 `oauth: true` 提醒用户首次连接时登录。使用有权限限制的预设，并避免把真实令牌、个人地址或机器专用路径提交到仓库。

提交前运行 `cargo test --locked -p codeneo-lite catalog_variants_use_supported_native_formats`。PR 请附上官方安装说明，以及实际试过的客户端和版本。若要支持一个新的 AI 客户端，需另加它的配置位置、传输方式、写入规则和测试；客户端定义在 [src-tauri/src/agents.rs](src-tauri/src/agents.rs)，配置目标在 [src-tauri/src/mcp/targets.rs](src-tauri/src/mcp/targets.rs)。

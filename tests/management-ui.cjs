const assert = require('node:assert/strict');
const path = require('node:path');
const { pathToFileURL } = require('node:url');
const { chromium } = require('playwright');
const { servers: mcpCatalog } = require('../mcp/catalog.json');

async function main() {
  const browser = await chromium.launch({
    headless: true,
    chromiumSandbox: true,
    ...(process.env.PW_EXECUTABLE_PATH ? { executablePath: process.env.PW_EXECUTABLE_PATH } : {}),
  });
  try {
    const page = await browser.newPage({ viewport: { width: 1080, height: 760 } });
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    await page.addInitScript(mcpCatalog => {
      const official = { state: 'official', account: 'Claude 官方', model: '', selected: null, can_undo: false, pending: null };
      const clients = [
        ['desktop', 'Claude Desktop', 'anthropic', ['stdio']],
        ['code', 'Claude Code', 'anthropic', ['http', 'stdio', 'sse']],
        ['codex', 'Codex', 'responses', ['http', 'stdio']],
        ['workbuddy', 'WorkBuddy', 'chat_completions', ['http', 'stdio', 'sse']],
      ].map(([id, name, protocol, transports]) => ({ id, name, protocol, transports, mcp_toggle: id === 'codex' || id === 'workbuddy', status: { ...official } }));
      clients[0].status = { ...official, state: 'third_party', account: '本机网关', model: 'qwen3.8-max' };
      clients[2].status.account = 'OpenAI 官方';
      clients[2].mcp_fields = [
        { key: 'cwd', label: '工作目录', kind: 'text', transport: 'stdio' },
        { key: 'bearer_token_env_var', label: 'Bearer Token 的环境变量名', kind: 'variable', transport: 'http' },
        { key: 'startup_timeout_sec', label: '启动超时（秒）', kind: 'timeout' },
      ];
      clients[3].status = { ...official, state: 'ready', account: '0 个自定义模型', model: '在 WorkBuddy 对话中选择' };
      clients[0].launch = { open: true, restart: true };
      clients[1].launch = { terminal: true };
      clients[2].launch = { open: true, terminal: true };
      clients[3].launch = { note: '未找到可启动的 WorkBuddy。安装后刷新状态。' };
      clients.push({
        id: 'qwen', name: 'Qwen Code', protocol: 'chat_completions',
        protocols: ['chat_completions', 'responses'], transports: ['http', 'stdio', 'sse'],
        agent: true, installed: true, launch: { terminal: true, version: '0.27.3' },
        status: { ...official, state: 'ready', account: '由客户端管理' },
      });
      clients.push({
        id: 'opencode', name: 'OpenCode', protocol: 'chat_completions', transports: ['http', 'stdio'],
        agent: true, installed: false, launch: { terminal: false, note: '未检测到 opencode 命令。' },
        help: 'https://opencode.ai/docs/config', install_help: true,
        status: { ...official, state: 'uninitialized', account: '尚无用户配置' },
      });
      window.launches = [];
      window.pickTerminalDirectoryCalls = 0;
      window.terminalDirectoryToPick = '/mock/project';
      window.prepareCodeEfforts = levels => {
        clients[1].status = { ...official, state: 'lite', selected: 'k3.1-pro-profile',
          model: 'k3.1-pro', code_efforts: levels };
      };
      window.queueCodeSwitch = id => { clients[1].status.pending = { id, issue: null }; };
      window.switches = [];
      window.gatewayAvailable = true;
      window.gatewayPaused = false;
      const hasGatewayRoute = () => clients.some(client => client.status.gateway_ready);
      const listeners = new Map();
      window.setGatewayPaused = paused => {
        window.gatewayPaused = paused;
        listeners.get('lite-gateway-changed')?.({});
      };
      const presets = [
        ['deepseek', 'DeepSeek', 'https://api.deepseek.com/anthropic', ['deepseek-flash', 'deepseek-v4-pro'], clients.map(client => client.id)],
        ['kimi', 'Kimi Code', 'https://api.kimi.com/coding', ['k3', 'k3.1-pro', 'k3.1-pro-highspeed', 'k3-256k', 'kimi-for-coding', 'kimi-for-coding-highspeed'], ['desktop', 'code']],
        ['openrouter', 'OpenRouter', 'https://openrouter.ai/api', ['anthropic/claude-sonnet-5.5', 'fixture/' + 'long-model-id-'.repeat(16) + 'v1'], clients.map(client => client.id)],
        ['glm', '智谱 GLM', 'https://open.bigmodel.cn/api/anthropic', ['glm-5.3', 'glm-5.3-flash'], ['desktop', 'code']],
        ['qwen', '通义千问', 'https://example.invalid/anthropic', ['qwen3.8-max', 'qwen3.8-flash', 'deepseek-v4.1-flash-1017', 'auto'], ['desktop', 'code']],
        ['openai', 'OpenAI API', 'https://api.openai.com/v1', ['gpt-6.1-sol'], ['codex', 'workbuddy']],
        ['custom', '自定义兼容接口', '', [], clients.map(client => client.id)],
      ].map(([id, name, base_url, models, clients]) => ({ id, provider: id, name, base_url, auth: 'auth_token',
        protocols: id === 'openai' ? ['responses', 'chat_completions'] : ['anthropic'], models, clients }));
      const view = {
        clients, presets,
        accounts: [{ id: 'missing-key', provider: 'deepseek', preset: 'deepseek', name: '待补 Key 的账号',
          has_key: false, clients: ['desktop', 'workbuddy'] }],
        models: [{ id: 'missing-key-profile', account: 'missing-key', name: '待补 Key 的模型',
          model: 'deepseek-flash', clients: ['desktop', 'workbuddy'] }],
      };
      window.keyChanges = [];
      window.addKeyAccount = () => {
        view.accounts.push({ ...view.accounts.find(item => item.id === 'kimi-account'), id: 'kimi-secondary', name: 'Kimi 备用接入' });
      };
      window.removeKeyAccount = () => { view.accounts = view.accounts.filter(item => item.id !== 'kimi-secondary'); };
      window.prepareLongModel = () => {
        const preset = presets.find(item => item.id === 'openrouter');
        view.accounts.push({ ...preset, id: 'openrouter-account', preset: preset.id, has_key: true });
        const id = preset.models[1];
        view.models.push({ id: 'long-profile', account: 'openrouter-account', name: '长名称模型', model: id, clients: preset.clients });
      };
      const mcp = { 'desktop/user': [{ name: 'frameleap', transport: 'stdio', enabled: true, permissions: { always_allow: [] } }] };
      clients[2].mcp_scopes = ['user', 'project'];
      const mcpConfigs = new Map();
      mcpConfigs.set('desktop/user/frameleap', { endpoint: '/fixture/frameleap', revision: 'frameleap-fixture' });
      const mcpUndo = new Map();
      let mcpRevision = 0;
      window.mcpWrites = [];
      window.mcpChecks = 0;
      window.permissionWrites = [];
      window.prepareAgentModels = () => {
        const clients = ['qwen', 'opencode'];
        view.accounts.push({ id: 'fixture-account', name: 'Fixture Provider', has_key: true, clients });
        for (const [id, name] of [['qa', 'Fixture A'], ['qb', 'Fixture B']]) {
          view.models.push({ id, name, model: id, account: 'fixture-account', clients });
        }
      };
      window.__TAURI__ = { core: { invoke: async (command, args) => {
        const client = clients.find(client => client.id === args?.client);
        if (command === 'lite_overview') {
          if (window.pendingOverview) await window.pendingOverview;
          const inUse = clients.flatMap(({ status, models_config }) => [
            status.selected, status.pending?.id, ...(models_config || []).map(item => item.id),
          ]);
          return structuredClone({ ...view, in_use: inUse.filter(id => view.models.some(model => model.id === id)) });
        }
        if (command === 'lite_gateway_status') return {
          paused: window.gatewayPaused,
          ready: !window.gatewayPaused && window.gatewayAvailable && hasGatewayRoute(),
        };
        if (command === 'lite_set_gateway_paused') {
          if (!args.paused) {
            if (!hasGatewayRoute()) throw '请先在 Claude Desktop 或 Codex 应用使用 Lite 网关的模型档案。';
            window.gatewayAvailable = true;
          }
          return window.setGatewayPaused(args.paused);
        }
        if (command === 'pick_terminal_directory') {
          window.pickTerminalDirectoryCalls++;
          return window.pendingTerminalDirectory ?? window.terminalDirectoryToPick;
        }
        if (command === 'open_agent_help') { window.openedAgentHelp = args; return; }
        if (command === 'apply_agent_models') {
          window.appliedAgentModels = args.models.map(item => item.id);
          client.models_config = args.models;
          client.status = { ...client.status, state: 'lite', model: args.models[0].id,
            models: window.appliedAgentModels, can_undo: true };
          return { message: '已应用模型列表。' };
        }
        if (command === 'launch_client') {
          if (!client.launch[args.action] || client.status.pending) throw '此客户端无法使用该启动方式';
          window.launches.push([client.id, args.action]);
          window.lastLaunchEffort = args.codeEffort;
          window.lastLaunchCwd = args.cwd;
          if (args.action === 'restart') return `${client.name} 已重启。`;
          return `已请求启动 ${client.name}。`;
        }
        if (command === 'switch_model_mode') {
          window.switches.push(args);
          window.lastSwitchCwd = args.cwd;
          const desktopRestart = client.id === 'desktop' && args.action === 'restart';
          const restarted = '配置已应用。Claude Desktop 已重启。';
          if (args.id === null) {
            client.status = { ...official, account: client.id === 'codex' ? 'OpenAI 官方' : official.account, can_undo: true };
            if (desktopRestart) window.launches.push([client.id, 'apply', null, args.action]);
            return desktopRestart ? restarted : client.name + ' 已切回官方模式。';
          }
          const model = view.models.find(model => model.id === args.id);
          if (!model?.clients.includes(client.id)) throw '模型不兼容';
          const local = args.connection === 'gateway';
          const hot = local && client.status.local_gateway && !args.action;
          const models = client.id === 'desktop' && local && model.desktop_models?.length ? model.desktop_models : [model.model];
          client.status = { ...official, state: 'lite', account: '测试服务商', model: models[0], models,
            selected: model.id, can_undo: true, local_gateway: local, gateway_ready: local };
          if (args.action) window.launches.push([client.id, 'apply', model.id, args.action]);
          if (hot) return `${client.name} 网关已切换模型。`;
          return desktopRestart ? restarted : client.name + ' 配置已写入。';
        }
        if (command === 'saved_model_catalog') {
          if (!view.models.some(model => model.id === args.id && model.clients.includes(client.id))) throw '错误的档案';
          return { models: [{ id: client.id === 'codex' ? 'fixture-codex-alt' : 'fixture-desktop-alt', name: '测试备用模型' }] };
        }
        if (command === 'verify_saved_model') {
          if (!view.models.some(model => model.id === args.id && model.clients.includes(client.id))) throw '错误的档案';
          return { healthy: true, message: '模型调用通过', latency_ms: 23 };
        }
        if (command === 'model_catalog') {
          if (!args.input.key && !view.accounts.some(account => account.id === args.input.account_id && account.has_key)) {
            throw '请填写 API Key';
          }
          window.catalogAccount = args.input.account_id;
          if (args.input.preset === 'qwen') {
            if (!window.qwenCatalogAvailable) throw '认证失败，请检查 API 密钥及所属平台（HTTP 401）';
            return { models: [{ id: 'qwen3.8-flash', name: '千问轻量版' }, { id: 'qwen3.8-max', name: '千问旗舰版' }] };
          }
          if (args.input.preset === 'deepseek') {
            return { models: [{ id: 'deepseek-flash', name: 'DeepSeek Flash' }, { id: 'deepseek-v4-pro', name: 'DeepSeek Pro' },
              ...Array.from({ length: 151 }, (_, index) => ({ id: `fixture-model-${index}`, name: `Fixture ${index}` }))] };
          }
          return { models: [{ id: 'k3.1-pro', name: 'Kimi K3.1 Pro' }, { id: 'k3', name: 'Kimi K3' }] };
        }
        if (command === 'test_model') {
          if (args.client !== 'desktop') throw '错误的客户端';
          window.probedModel = args.input.model;
          return { healthy: true, message: '模型调用通过', latency_ms: 42 };
        }
        if (command === 'update_account_key') {
          const account = view.accounts.find(item => item.id === args.accountId);
          if (!account || args.key !== 'fixture-new-key') throw '请填写有效的模型 API Key';
          window.keyChanges.push(account.id);
          account.has_key = true;
          return '新 API Key 已保存。直连客户端需重新应用此账号的模型档案。';
        }
        if (command === 'save_model' || command === 'save_models') {
          const preset = presets.find(item => item.id === args.input.preset);
          const account = args.input.account_id
            ? view.accounts.find(item => item.id === args.input.account_id)
            : { ...preset, id: preset.id + '-account' + (args.input.account_name ? '-' + args.input.account_name : ''),
                name: args.input.account_name || preset.name, preset: preset.id, protocol: args.input.protocol, has_key: true };
          if (args.input.key) account.has_key = true;
          if (!args.input.account_id) view.accounts.push(account);
          const id = args.input.id || args.input.model + '-profile';
          const old = view.models.find(model => model.id === id);
          const saved = { id, account: account.id, name: args.input.name || args.input.model, model: args.input.model, clients: account.clients,
            desktop_models: args.input.desktop_models ?? old?.desktop_models ?? [], code_models: args.input.code_models ?? old?.code_models ?? {} };
          if (saved.clients.includes('desktop')) {
            const desktopModels = saved.desktop_models.length ? saved.desktop_models : [saved.model];
            saved.desktop_direct = desktopModels.every(id => /^(anthropic\/)?claude-/i.test(id));
          }
          view.models = [...view.models.filter(model => model.id !== id), saved];
          if (args.input.id) window.editedProfile = { id, model: saved.model, desktopModels: saved.desktop_models };
          if (command === 'save_model') return id;
          window.batchSaves = [...(window.batchSaves || []), {
            account: account.id, client: args.client, models: [saved.model, ...args.selected.map(entry => entry.model)],
          }];
          const ids = [id];
          if (args.client !== 'desktop') {
            for (const entry of args.selected) {
              const entryId = entry.model + '-profile';
              view.models = [...view.models.filter(model => model.id !== entryId), {
                id: entryId, account: account.id, model: entry.model, name: entry.name || entry.model, clients: account.clients,
              }];
              ids.push(entryId);
            }
          }
          return ids;
        }
        if (command === 'apply_workbuddy_models') {
          window.workbuddyModels = args.ids;
          return { message: `已向 WorkBuddy 添加 ${args.ids.length} 个模型。` };
        }
        if (command === 'remove_model') {
          view.models = view.models.filter(model => model.id !== args.id);
          const code = clients.find(client => client.id === 'code');
          if (code.status.pending?.id === args.id) code.status.pending = null;
          return null;
        }
        if (command === 'apply_model') {
          const model = view.models.find(model => model.id === args.id);
          if (!model?.clients.includes(client.id)) throw '模型不兼容';
          if (client.id === 'code') {
            client.status.pending = { id: model.id, issue: null };
            return { message: 'Claude Code 正在运行，已等待会话退出后应用。' };
          }
          if (client.id === 'workbuddy') {
            client.status.account = '1 个自定义模型';
            return { message: '已加入 WorkBuddy 自定义模型。' };
          }
          client.status = { state: 'lite', account: '测试服务商', model: model.desktop_models?.[0] || model.model,
            models: model.desktop_models, local_gateway: !!model.desktop_models?.length, selected: model.id, can_undo: true };
          return { message: client.name + ' 配置已写入。' };
        }
        if (command === 'undo_model') {
          client.status = { ...official };
          return { message: '已撤销上次 Desktop 切换。' };
        }
        if (command === 'cancel_code_switch') { client.status.pending = null; return null; }
        if (command === 'list_mcp_catalog') return mcpCatalog.flatMap(card => {
          const variant = card.variants.find(v => client.transports.includes(v.transport) && (!v.clients || v.clients.includes(client.id)));
          if (!variant) return [];
          const { input, options, ...rest } = variant;
          return [{ ...card, ...rest, input: input && { label: input.label, required: input.required, directory: input.target === 'directory', project_variable: true } }];
        });
        if (command === 'pick_mcp_directory') return '/fixture/Project With Spaces';
        if (command === 'prepare_mcp_preset') {
          const variant = mcpCatalog.find(item => item.id === args.id).variants
            .find(v => client.transports.includes(v.transport) && (!v.clients || v.clients.includes(client.id)));
          const options = { ...variant.options };
          const launchArgs = [...(variant.args || [])];
          const advanced = {};
          if (variant.input?.target === 'directory') launchArgs.push(args.value);
          else if (args.scope === 'project' && client.id === 'codex' && args.value) advanced.bearer_token_env_var = args.value;
          else if (args.value) options[variant.input.key] = (variant.input.prefix || '') + args.value;
          return { name: args.id, transport: variant.transport, endpoint: variant.endpoint, options, args: launchArgs, advanced };
        }
        if (command === 'parse_mcp_import') {
          if (!args.text.includes('custom-service')) throw '片段格式错误';
          return { name: 'custom-service', transport: 'http', endpoint: 'https://example.invalid/custom', options: {}, args: [], advanced: {}, skipped: ['trust'] };
        }
        if (command === 'list_mcp') {
          const { scope, directory } = args.selection;
          const id = [client.id, scope, directory].filter(Boolean).join('/');
          const configPath = scope === 'project' ? directory + '/.codex/config.toml' : '/fixture/' + client.id + '/mcp-config';
          const servers = (mcp[id] || []).map(server => ({
            ...server, revision: mcpConfigs.get(`${id}/${server.name}`)?.revision,
          }));
          return { location: id, label: scope === 'project' ? '项目共享' : client.id === 'desktop' ? '第三方模型模式' : '用户级',
            path: configPath, can_undo: mcpUndo.has(id), servers };
        }
        if (command === 'frameleap_status') return { mode: 'gateway', state: 'existing' };
        const destination = args?.destination?.location;
        if (command === 'get_mcp') {
          return structuredClone(mcpConfigs.get(`${destination}/${args.name}`));
        }
        if (command === 'verify_mcp') {
          const { source, name, input } = args.target;
          const config = source === 'draft' ? input : mcpConfigs.get(`${destination}/${name}`);
          window.mcpChecks++;
          await new Promise(resolve => setTimeout(resolve, 150));
          if (!config) throw '找不到此 MCP 服务，请刷新列表';
          const result = config.endpoint.endsWith('/denied')
            ? { Err: 'MCP 认证失败（HTTP 401），请检查凭据或服务权限' }
            : { Ok: { protocol_version: '2025-11-25', tool_count: 3, tool_names: ['list_projects', 'get_project', 'render_video'] } };
          return { revision: config.revision, result };
        }
        if (command === 'save_mcp_permissions') {
          const config = mcpConfigs.get(`${destination}/${args.name}`);
          if (config?.revision !== args.revision) throw '服务配置已变化';
          const server = mcp[destination].find(item => item.name === args.name);
          server.permissions.always_allow = args.alwaysAllow;
          config.revision = String(++mcpRevision);
          window.permissionWrites.push(args.alwaysAllow);
          return null;
        }
        if (command === 'save_mcp') {
          const { name, transport, revision } = args.input;
          const key = `${destination}/${name}`;
          mcp[destination] ||= [];
          const existing = mcp[destination].find(item => item.name === name);
          if (existing) {
            if (revision !== mcpConfigs.get(key)?.revision) throw 'MCP 配置已变化，请重新打开编辑器';
            existing.transport = transport;
          } else {
            mcp[destination].push({ name, transport, enabled: true });
            mcpUndo.set(destination, name);
          }
          const advanced = { ...mcpConfigs.get(key)?.advanced, ...args.input.advanced };
          mcpConfigs.set(key, { ...structuredClone(args.input), advanced, revision: String(++mcpRevision) });
          window.mcpWrites.push({ destination, ...structuredClone(args.input) });
          return null;
        }
        if (command === 'undo_mcp') {
          const name = mcpUndo.get(destination);
          mcp[destination] = mcp[destination].filter(item => item.name !== name);
          mcpConfigs.delete(`${destination}/${name}`);
          mcpUndo.delete(destination);
          return null;
        }
        if (command === 'remove_mcp') {
          mcp[destination] = mcp[destination].filter(item => item.name !== args.name);
          mcpConfigs.delete(`${destination}/${args.name}`);
          return null;
        }
        if (command === 'set_mcp_enabled') {
          mcp[destination].find(item => item.name === args.name).enabled = args.enabled;
          return null;
        }
        if (command === 'token_usage') {
          if (![1, 7, 30].includes(args.days)) throw '无效时间范围';
          const largeUsage = [
            { client: 'codex', model: 'gpt-6.1-sol', tokens: { input: 19797476, output: 3692442, cache_read: 1390062208, cache_write: 0, cache_write_long: 0 },
              estimate: { low_cny: 2202.6629, high_cny: 2202.6629, source: 'openai' } },
            { client: 'code', model: 'qwen3.8-max', tokens: { input: 321908, output: 1728598, cache_read: 246064647, cache_write: 8298055, cache_write_long: 0 },
              estimate: { low_cny: 436.6279, high_cny: 559.6602, source: 'qwen_max' } },
            { client: 'code', model: 'qwen4.0-max-preview', tokens: { input: 103776, output: 263214, cache_read: 52673727, cache_write: 1945866, cache_write_long: 0 }, estimate: null,
              unpriced: [{ reason: 'plan_only', source: 'qwen_preview' }] },
            { client: 'codex', model: 'codex-auto-review', tokens: { input: 4495462, output: 62816, cache_read: 29899008, cache_write: 0, cache_write_long: 0 }, estimate: null,
              unpriced: [{ reason: 'dynamic_model', source: null }] },
            { client: 'desktop', model: 'qwen3.8-max', tokens: { input: 318, output: 22414, cache_read: 3023462, cache_write: 536913, cache_write_long: 0 },
              estimate: { low_cny: 11.8879, high_cny: 13.3996, source: 'qwen_max' } },
          ];
          return {
            app_version: '0.1.8', pricing_verified: '2026-11-17',
            pricing_sources: [
              { id: 'openai', label: 'OpenAI 官方价', url: 'https://developers.openai.com/api/docs/pricing', verified: '2026-11-13' },
              { id: 'qwen_max', label: '百炼官方价 · 北京', url: 'https://help.aliyun.com/zh/model-studio/qwen3-8-max', verified: '2026-11-13' },
              { id: 'qwen4_max', label: '百炼官方价 · 北京', url: 'https://help.aliyun.com/zh/model-studio/model-pricing', verified: '2026-11-17' },
              { id: 'glm', label: '智谱官方价', url: 'https://docs.bigmodel.cn/cn/guide/start/pricing', verified: '2026-11-16' },
              { id: 'qwen_preview', label: 'Token Plan 官方说明', url: 'https://help.aliyun.com/zh/model-studio/qwen4-max-preview', verified: '2026-11-16' },
            ],
            exchange_rate_date: '2026-11-12', usd_to_cny: 6.7174,
            exchange_rate_source: 'https://xinhuanet.com.cn/20261112/97116e6012634afcaefaa35c8e5f9a92/c.html',
            rows: window.usageRows || (args.days === 30 ? largeUsage : args.days === 1 ? [] : [
              { client: 'codex', model: 'gpt-6.1-sol', tokens: { input: 250, output: 500, cache_read: 1000, cache_write: 0, cache_write_long: 0 },
                estimate: { low_cny: 0.03761744, high_cny: 0.03761744, source: 'openai' } },
              { client: 'desktop', model: 'qwen4-max', tokens: { input: 500, output: 150, cache_read: 100, cache_write: 30, cache_write_long: 20 },
                estimate: { low_cny: 0.010325, high_cny: 0.010325, source: 'qwen4_max' } },
              { client: 'code', model: 'glm-5.3', tokens: { input: 70, output: 10, cache_read: 0, cache_write: 0, cache_write_long: 0 },
                estimate: { low_cny: 0.00084, high_cny: 0.00084, source: 'glm' } },
            ]),
            coverage: clients.map(({ id }) => ({ client: id, state: id === 'workbuddy' ? 'unsupported'
              : args.days === 1 ? 'empty' : args.days === 30 && id === 'codex' ? 'partial' : 'available' })),
          };
        }
        throw new Error('Unexpected operation ' + command);
      } }, event: { listen: async (name, listener) => {
        listeners.set(name, listener);
        return () => listeners.delete(name);
      } } };
    }, mcpCatalog);
    await page.goto(pathToFileURL(path.join(__dirname, '../ui/index.html')).href);
    const nav = name => page.getByRole('navigation').getByRole('button', { name, exact: true });
    const client = name => page.locator('#client-list').getByRole('button', { name, exact: true });
    const restartDialog = page.getByRole('dialog', { name: '重启 Claude Desktop？', exact: true });
    const confirmRestart = () => restartDialog.getByRole('button', { name: '确认重启', exact: true }).click();
    const gatewayIndicator = ready => page.getByRole('img', { name: ready ? '网关正常' : '网关未就绪', exact: true });
    const startGateway = page.getByRole('button', { name: '启动网关', exact: true });
    const pauseGateway = page.getByRole('button', { name: '暂停网关', exact: true });
    const capture = async (name, options = {}) => {
      if (process.env.LITE_QA_DIR) await page.screenshot({ path: path.join(process.env.LITE_QA_DIR, name + '.png'), ...options });
    };

    await page.getByRole('heading', { name: 'Claude Desktop', exact: true }).waitFor();
    await gatewayIndicator(false).waitFor();
    await startGateway.click();
    await page.locator('#gateway-feedback').getByText('请先在 Claude Desktop 或 Codex 应用使用 Lite 网关的模型档案。', { exact: true }).waitFor();
    assert.equal(await page.getByRole('combobox', { name: '连接方式', exact: true }).inputValue(), 'gateway');
    assert.equal(await page.locator('#current-model').innerText(), 'qwen3.8-max');
    await page.getByRole('button', { name: '打开 Claude Desktop', exact: true }).click();
    await page.getByText('已请求启动 Claude Desktop。', { exact: true }).waitFor();
    await page.getByRole('button', { name: '重启 Claude Desktop', exact: true }).click();
    await restartDialog.getByRole('button', { name: '取消', exact: true }).click();
    assert.deepEqual(await page.evaluate(() => window.launches), [['desktop', 'open']]);
    await page.getByRole('button', { name: '重启 Claude Desktop', exact: true }).click();
    await confirmRestart();
    await page.locator('#launch-feedback').getByText('Claude Desktop 已重启。', { exact: true }).waitFor();
    assert.deepEqual(await page.evaluate(() => window.launches), [['desktop', 'open'], ['desktop', 'restart']]);
    await capture('01-clients');
    const searchToggle = page.getByRole('button', { name: '搜索客户端', exact: true });
    const clientSearch = page.getByRole('searchbox', { name: '搜索客户端', exact: true });
    assert.equal(await clientSearch.isVisible(), false);
    await searchToggle.click();
    assert.equal(await clientSearch.evaluate(input => input === document.activeElement), true);
    await clientSearch.fill('work');
    assert.equal(await page.locator('#client-list button').count(), 1);
    await clientSearch.press('Escape');
    assert.equal(await searchToggle.getAttribute('aria-expanded'), 'false');
    assert.equal(await searchToggle.evaluate(button => button === document.activeElement), true);
    assert.equal(await page.locator('#client-list button').count(), 7);
    await client('OpenCode').click();
    assert.equal(await page.locator('#client-status').innerText(), '未检测到安装');
    assert.ok(!(await page.locator('#terminal-client').isVisible()));
    await page.getByRole('button', { name: '查看安装说明 ↗', exact: true }).click();
    assert.deepEqual(await page.evaluate(() => window.openedAgentHelp), { client: 'opencode', install: true });
    await client('Claude Desktop').click();

    await nav('模型库').click();
    const provider = name => page.getByRole('article', { name, exact: true });
    const providerFilter = name => page.locator('#library-providers').getByRole('button', { name, exact: true });
    const modelSearch = page.getByLabel('搜索模型', { exact: true });
    const libraryClient = page.getByLabel('筛选客户端');
    const libraryProvider = page.getByLabel('筛选服务商');
    const catalogTab = page.getByRole('button', { name: '模型目录', exact: true });
    const savedTab = page.getByRole('button', { name: /^已配置/ });
    const keyEditor = page.getByRole('dialog', { name: '修改 API Key', exact: true });
    const newKey = keyEditor.getByLabel('新 API Key', { exact: true });
    const keyAccount = keyEditor.getByLabel('接入账号', { exact: true });
    const saveNewKey = keyEditor.getByRole('button', { name: '保存新 Key', exact: true });
    await page.getByRole('heading', { name: '还没有配置模型' }).waitFor();
    await capture('02-library');
    assert.equal(await page.locator('#library-summary').innerText(), '0 个已配置模型');
    assert.equal(await page.locator('.provider-panel, .provider-model').count(), 0);
    assert.equal(await page.locator('#saved-count').innerText(), '0');
    await page.getByRole('button', { name: '补填 API Key', exact: true }).click();
    assert.equal(await keyAccount.inputValue(), 'missing-key');
    assert.equal(await newKey.inputValue(), '');
    assert.ok(await saveNewKey.isDisabled());
    await newKey.fill('fixture-unsaved');
    await newKey.press('Escape');
    await keyEditor.waitFor({ state: 'hidden' });
    await page.waitForFunction(() => document.getElementById('new-account-key').value === '');
    assert.deepEqual(await page.evaluate(() => window.keyChanges), []);
    assert.equal(await modelSearch.evaluate(input => input.getBoundingClientRect().height),
      await libraryClient.evaluate(select => select.getBoundingClientRect().height));
    await savedTab.click();
    await page.getByRole('heading', { name: '还没有配置模型' }).waitFor();
    assert.equal(await page.locator('.saved-model').count(), 0);
    await page.locator('#add-first-model').click();
    await page.locator('#provider-picker').selectOption('deepseek');
    await page.locator('#account-picker').selectOption('missing-key');
    const key = page.getByLabel('模型 API Key', { exact: true });
    assert.equal(await key.getAttribute('type'), 'password');
    assert.ok(await page.getByRole('button', { name: '加载模型列表' }).isDisabled());
    assert.ok(await page.getByRole('button', { name: '手动选择模型' }).isDisabled());
    assert.equal(await page.locator('#candidate-list [data-model]').count(), 0);
    await page.getByRole('button', { name: '返回模型库', exact: true }).click();
    assert.equal(await page.locator('#add-first-model').evaluate(control => document.activeElement === control), true);

    await page.locator('#add-first-model').click();
    await page.locator('#provider-picker').selectOption('qwen');
    await key.fill('sk-sp-fixture-only');
    await page.getByRole('button', { name: '加载模型列表' }).click();
    await page.getByRole('status').filter({ hasText: 'HTTP 401' }).waitFor();
    const candidateSearch = page.getByLabel('搜索可选模型');
    const candidateIds = () => page.locator('#candidate-list [data-model]').evaluateAll(rows => rows.map(row => row.dataset.model));
    const selectedList = page.locator('#selected-model-list');
    const selectedIds = () => selectedList.locator('code').allTextContents();
    const defaultRow = selectedList.getByRole('listitem').filter({ has: page.getByText('默认', { exact: true }) });
    const defaultId = () => defaultRow.locator('code').textContent();
    await candidateSearch.fill('qwen 3.8 FLASH');
    assert.deepEqual(await candidateIds(), ['qwen3.8-flash']);
    await candidateSearch.fill('ＤｅｅｐＳｅｅｋ　flash');
    assert.deepEqual(await candidateIds(), ['deepseek-v4.1-flash-1017']);
    await page.getByRole('checkbox', { name: 'deepseek-v4.1-flash-1017', exact: true }).check();
    await candidateSearch.press('Enter');
    assert.ok(await candidateSearch.isVisible(), 'Searching with Enter must not save the model');
    assert.equal(await defaultId(), 'deepseek-v4.1-flash-1017');
    await candidateSearch.fill('轻量');
    await page.getByText('当前列表没有匹配的模型。', { exact: true }).waitFor();
    await page.evaluate(() => { window.qwenCatalogAvailable = true; });
    await page.locator('#refresh-catalog').click();
    await page.getByText('已读取 2 个模型。', { exact: true }).waitFor();
    assert.equal(await candidateSearch.inputValue(), '轻量');
    assert.deepEqual(await candidateIds(), ['qwen3.8-flash']);
    assert.deepEqual(await selectedIds(), ['deepseek-v4.1-flash-1017']);
    await candidateSearch.fill('');
    assert.deepEqual(await candidateIds(), ['qwen3.8-flash', 'qwen3.8-max']);
    await page.getByRole('button', { name: '返回模型库', exact: true }).click();
    assert.equal(await page.locator('#saved-count').innerText(), '0');

    await nav('客户端').click();
    await page.getByRole('button', { name: '配置新模型' }).click();
    await page.locator('#provider-picker').selectOption('kimi');
    assert.ok(await page.getByRole('button', { name: '加载模型列表' }).isDisabled());
    await key.fill('fixture-model-key');
    await capture('03-credentials');
    await page.getByRole('button', { name: '加载模型列表' }).click();
    await page.getByText('已读取 2 个模型。', { exact: true }).waitFor();
    await page.getByRole('checkbox', { name: /Kimi K3.1 Pro/ }).check();
    assert.equal(await defaultId(), 'k3.1-pro');
    await candidateSearch.fill('k3');
    await page.getByRole('checkbox', { name: 'Kimi K3 k3', exact: true }).check();
    await candidateSearch.fill('k3.1');
    await page.getByRole('button', { name: '保存 2 个模型', exact: true }).waitFor();
    await candidateSearch.fill('');
    assert.equal(await page.getByRole('checkbox', { name: /Kimi K3.1 Pro/ }).isChecked(), true);
    assert.equal(await page.getByRole('checkbox', { name: 'Kimi K3 k3', exact: true }).isChecked(), true);
    const saveAction = await page.getByRole('button', { name: '保存 2 个模型', exact: true }).boundingBox();
    assert.ok(saveAction.y + saveAction.height <= 760, 'Model editor actions should stay in view');
    await capture('04-select');
    await page.getByRole('button', { name: '测试连接' }).click();
    await page.getByText('模型调用通过，耗时 42 毫秒。', { exact: true }).waitFor();
    await page.getByLabel('档案名称（可选）').fill('Kimi 工作档');
    await page.getByRole('button', { name: '保存 2 个模型', exact: true }).click();
    await page.getByText('档案已保存。选择连接方式后，点击「应用模式」。').waitFor();
    assert.deepEqual(await page.evaluate(() => window.batchSaves[0]), {
      account: 'kimi-account', client: 'desktop', models: ['k3.1-pro', 'k3'],
    });
    assert.equal(await page.locator('#current-model').innerText(), 'qwen3.8-max');
    assert.equal(await page.locator('#client-connection').inputValue(), 'gateway');
    assert.equal(await page.locator('#client-connection option[value="direct"]').isDisabled(), true);
    assert.ok(await page.getByRole('button', { name: '应用模式', exact: true }).isDisabled());
    const applyAndRestart = page.getByRole('button', { name: '应用并重启', exact: true });
    const restarted = page.getByText('配置已应用。Claude Desktop 已重启。', { exact: true });
    await applyAndRestart.click();
    await restartDialog.press('Escape');
    await restartDialog.waitFor({ state: 'hidden' });
    assert.deepEqual(await page.evaluate(() => window.switches), []);
    await applyAndRestart.click();
    await confirmRestart();
    await restarted.waitFor();
    assert.deepEqual(await page.evaluate(() => window.switches.at(-1)), {
      client: 'desktop', id: 'k3.1-pro-profile', connection: 'gateway', action: 'restart',
    });
    assert.deepEqual(await page.locator('#current-models code').allTextContents(), ['默认 · k3.1-pro', 'k3']);
    assert.equal(await page.locator('#gateway-status').innerText(), 'CodeNeo Lite网关 · 已就绪');
    await gatewayIndicator(true).waitFor();
    for (const colorScheme of ['light', 'dark']) {
      await page.emulateMedia({ colorScheme });
      const color = await gatewayIndicator(true).evaluate(dot => getComputedStyle(dot).backgroundColor);
      const [red, green, blue] = color.match(/\d+/g).map(Number);
      assert.ok(green > red && green > blue, `Ready gateway uses a green dot in ${colorScheme} mode`);
    }
    await page.emulateMedia({ colorScheme: null });
    await capture('gateway-ready');
    const pausedIndicator = page.getByRole('img', { name: '网关已暂停', exact: true });
    await pauseGateway.click();
    await startGateway.waitFor();
    await pausedIndicator.waitFor();
    assert.equal(await page.locator('#gateway-status').innerText(), 'CodeNeo Lite网关 · 已暂停');
    await capture('gateway-paused');
    await startGateway.click();
    await gatewayIndicator(true).waitFor();
    await page.evaluate(() => window.setGatewayPaused(true));
    await startGateway.waitFor();
    await page.evaluate(() => window.setGatewayPaused(false));
    await pauseGateway.waitFor();
    await page.evaluate(() => { window.gatewayAvailable = false; });
    await gatewayIndicator(false).waitFor({ timeout: 8000 });
    await startGateway.click();
    await gatewayIndicator(true).waitFor();
    await page.getByRole('button', { name: '拉取模型', exact: true }).click();
    await page.getByText('供应商模型目录（1 项）', { exact: true }).waitFor();
    assert.equal(await page.locator('#client-catalog-model').inputValue(), 'fixture-desktop-alt');
    page.once('dialog', dialog => dialog.accept());
    await page.getByRole('button', { name: '验证供应商', exact: true }).click();
    await page.getByText('模型调用通过，耗时 23 毫秒。', { exact: true }).waitFor();
    assert.equal(await key.inputValue(), '');
    await nav('模型库').click();
    await page.getByRole('heading', { name: 'Kimi 工作档' }).waitFor();
    assert.equal(await page.locator('#saved-count').innerText(), '2');
    await page.locator('.saved-profile-models').getByText('k3', { exact: true }).waitFor();
    assert.ok(await page.getByRole('button', { name: '删除', exact: true }).isEnabled());
    await page.getByText('使用中', { exact: true }).waitFor();
    await page.getByRole('button', { name: '修改 API Key', exact: true }).click();
    assert.equal(await keyAccount.inputValue(), 'kimi-account');
    assert.equal(await newKey.getAttribute('type'), 'password');
    assert.equal(await newKey.inputValue(), '');
    await newKey.fill('fixture-invalid');
    await saveNewKey.click();
    await keyEditor.getByText('请填写有效的模型 API Key', { exact: true }).waitFor();
    await keyEditor.getByRole('button', { name: '取消', exact: true }).click();
    await libraryProvider.selectOption('kimi');
    await modelSearch.fill('Kimi 工作档');
    assert.equal(await page.locator('.saved-model').count(), 1);
    await capture('09-library-saved');
    await page.getByRole('button', { name: '编辑', exact: true }).click();
    await page.getByRole('heading', { name: '服务商模型列表' }).waitFor();
    assert.equal(await page.locator('#model-id').inputValue(), 'k3.1-pro');
    await page.getByRole('button', { name: '返回模型库', exact: true }).click();
    assert.equal(await modelSearch.inputValue(), 'Kimi 工作档');

    await catalogTab.click();
    await providerFilter('Kimi Code').click();
    assert.equal(await page.locator('#library-summary').innerText(), '2 个已配置模型');
    assert.deepEqual(await page.locator('.provider-model code').allTextContents(), ['k3.1-pro', 'k3']);
    assert.equal(await provider('DeepSeek').count(), 0);
    await page.evaluate(async () => { window.addKeyAccount(); await lite.refresh(); });
    await provider('Kimi Code').getByRole('button', { name: '修改 Kimi Code 的 API Key', exact: true }).click();
    assert.equal(await keyAccount.inputValue(), '');
    assert.equal(await newKey.inputValue(), '');
    await newKey.fill('fixture-unsaved');
    assert.ok(await saveNewKey.isDisabled());
    await keyAccount.selectOption('kimi-secondary');
    assert.equal(await newKey.inputValue(), '');
    await newKey.fill('fixture-new-key');
    await saveNewKey.click();
    await page.locator('#models-feedback').getByText('新 API Key 已保存。直连客户端需重新应用此账号的模型档案。', { exact: true }).waitFor();
    assert.deepEqual(await page.evaluate(() => window.keyChanges), ['kimi-secondary']);
    assert.deepEqual(await page.locator('.provider-model code').allTextContents(), ['k3.1-pro', 'k3']);
    await page.evaluate(async () => { window.removeKeyAccount(); await lite.refresh(); });
    const compatibility = provider('Kimi Code').getByRole('button', { name: '兼容客户端' });
    await compatibility.click();
    await provider('Kimi Code').getByText('Claude Desktop · Claude Code', { exact: true }).waitFor();
    await modelSearch.fill('kimi pro');
    assert.equal(await provider('Kimi Code').locator('.provider-model').count(), 1);
    assert.equal(await compatibility.getAttribute('aria-expanded'), 'true');
    await libraryClient.selectOption('codex');
    await page.getByText('Kimi Code 没有适用于 Codex 的已配置模型。', { exact: true }).waitFor();
    await page.getByRole('button', { name: '清除筛选' }).click();
    await provider('Kimi Code').getByRole('button', { name: '查看已配置模型 k3', exact: true }).click();
    await page.getByRole('heading', { name: 'Kimi 工作档' }).waitFor();
    await catalogTab.click();
    await provider('Kimi Code').getByRole('button', { name: '拉取 Kimi Code 模型', exact: true }).click();
    await page.getByText('已读取 2 个模型。', { exact: true }).waitFor();
    assert.equal(await page.evaluate(() => window.catalogAccount), 'kimi-account');
    assert.equal(await key.inputValue(), '');
    assert.equal(await page.locator('#account-picker').inputValue(), 'kimi-account');
    await page.getByRole('button', { name: '上一步' }).click();
    await page.locator('#account-picker').selectOption('');
    assert.ok(await page.getByRole('button', { name: '手动选择模型' }).isDisabled());
    await page.locator('#account-picker').selectOption('kimi-account');
    await page.getByRole('button', { name: '手动选择模型' }).click();
    await page.getByRole('checkbox', { name: 'k3-256k', exact: true }).check();
    await page.getByRole('button', { name: '保存档案' }).click();
    const savedKimi = page.locator('.saved-model').filter({ has: page.getByRole('heading', { name: 'k3-256k', exact: true }) });
    await savedKimi.waitFor();
    const removeKimiDialog = page.getByRole('dialog', { name: '删除「k3-256k」？', exact: true });
    await savedKimi.getByRole('button', { name: '删除', exact: true }).click();
    await removeKimiDialog.getByRole('button', { name: '取消', exact: true }).click();
    assert.ok(await savedKimi.isVisible());
    await savedKimi.getByRole('button', { name: '删除', exact: true }).click();
    await page.evaluate(() => {
      window.pendingOverview = new Promise(resolve => { window.resumeOverview = resolve; });
    });
    await removeKimiDialog.getByRole('button', { name: '确认删除', exact: true }).click();
    await savedKimi.waitFor({ state: 'detached' });
    await page.getByText('模型档案已删除。', { exact: true }).waitFor();
    await page.evaluate(() => {
      window.resumeOverview();
      delete window.pendingOverview;
    });

    await catalogTab.click();
    assert.equal(await provider('Kimi Code').getByText('k3-256k', { exact: true }).count(), 0);
    await providerFilter('Kimi Code').click();
    await page.locator('#new-model').click();
    await page.locator('#account-picker').selectOption('');
    await key.fill('fixture-second-key');
    await page.getByText('接入名称（可选）', { exact: true }).click();
    await page.getByLabel('接入名称', { exact: true }).fill('测试备用账号');
    await page.getByRole('button', { name: '手动选择模型', exact: true }).click();
    await page.getByRole('checkbox', { name: 'k3', exact: true }).check();
    await page.getByRole('button', { name: '保存档案', exact: true }).click();
    const standaloneK3 = page.locator('.saved-model').filter({ has: page.getByRole('heading', { name: 'k3', exact: true }) });
    await standaloneK3.getByText('Kimi Code · 测试备用账号', { exact: true }).waitFor();
    await catalogTab.click();
    await provider('Kimi Code').getByRole('button', { name: '查看已配置模型 k3', exact: true }).waitFor();
    await provider('Kimi Code').getByRole('button', { name: '拉取 Kimi Code 模型', exact: true }).click();
    assert.equal(await page.locator('#account-picker').inputValue(), '');
    await page.locator('#account-picker').selectOption({ label: '测试备用账号' });
    await page.getByRole('button', { name: '手动选择模型', exact: true }).click();
    await page.getByRole('checkbox', { name: 'k3.1-pro-highspeed', exact: true }).check();
    assert.equal(await defaultId(), 'k3.1-pro-highspeed');
    await page.getByRole('button', { name: '返回模型库', exact: true }).click();
    await savedTab.click();
    await modelSearch.fill('k3');
    const kimiProfile = page.locator('.saved-model').filter({ has: page.getByRole('heading', { name: 'Kimi 工作档', exact: true }) });
    assert.ok(await kimiProfile.getByRole('button', { name: '删除', exact: true }).isEnabled());
    await standaloneK3.getByRole('button', { name: '删除', exact: true }).click();
    await page.getByRole('dialog', { name: '删除「k3」？', exact: true })
      .getByRole('button', { name: '确认删除', exact: true }).click();
    await standaloneK3.waitFor({ state: 'detached' });
    assert.equal(await modelSearch.inputValue(), 'k3');
    await kimiProfile.getByText('k3', { exact: true }).waitFor();

    await nav('客户端').click();
    await page.getByText('恢复与撤销', { exact: true }).click();
    await page.getByRole('button', { name: '撤销上次配置', exact: true }).click();
    await page.getByText('已撤销上次 Desktop 切换。').waitFor();
    const restartToApply = page.getByRole('button', { name: '重启以生效', exact: true });
    await restartToApply.click();
    await confirmRestart();
    await restartToApply.waitFor({ state: 'hidden' });
    assert.deepEqual(await page.evaluate(() => window.launches.at(-1)), ['desktop', 'restart']);
    await page.getByLabel('运行模式').selectOption('third_party');
    await page.locator('#client-model-picker').selectOption('k3.1-pro-profile');
    await applyAndRestart.click();
    await confirmRestart();
    await restarted.waitFor();
    assert.deepEqual(await page.evaluate(() => window.launches.at(-1)), ['desktop', 'apply', 'k3.1-pro-profile', 'restart']);
    await page.getByLabel('运行模式').selectOption('official');
    await applyAndRestart.click();
    await confirmRestart();
    await restarted.waitFor();
    assert.deepEqual(await page.evaluate(() => window.switches.at(-1)), {
      client: 'desktop', id: null, connection: null, action: 'restart',
    });
    await client('Claude Code').click();
    const projectPanel = page.getByRole('group', { name: '项目目录', exact: true });
    const chooseProject = projectPanel.getByRole('button', { name: '选择目录…', exact: true });
    const launchCode = page.getByRole('button', { name: '在终端启动', exact: true });
    const codeProject = '/mock/Claude Code/中文项目';
    await projectPanel.getByText('尚未选择', { exact: true }).waitFor();
    assert.ok(await launchCode.isDisabled());
    const launchesBeforeCodeSelection = await page.evaluate(() => window.launches.length);
    await page.evaluate(directory => { window.terminalDirectoryToPick = directory; }, codeProject);
    await chooseProject.click();
    await projectPanel.getByText(codeProject, { exact: true }).waitFor();
    assert.equal(await page.evaluate(() => window.launches.length), launchesBeforeCodeSelection);
    await page.getByLabel('运行模式').selectOption('third_party');
    await page.locator('#client-model-picker').selectOption('k3.1-pro-profile');
    await page.getByRole('button', { name: '配置模型映射', exact: true }).click();
    await page.getByLabel('Sonnet', { exact: true }).fill('k3');
    await page.getByLabel('子 Agent', { exact: true }).fill('k3');
    await page.getByRole('button', { name: '保存并应用', exact: true }).click();
    await page.getByText('等待 Claude Code 会话退出后应用 Kimi 工作档。', { exact: true }).waitFor();
    assert.ok(await launchCode.isDisabled());
    await page.getByRole('button', { name: '取消等待', exact: true }).click();
    await page.getByText('已取消等待中的切换。').waitFor();
    await launchCode.click();
    await page.getByText('已请求启动 Claude Code。', { exact: true }).waitFor();
    const codeEffort = page.getByLabel('新会话思考强度');
    assert.ok(!(await codeEffort.isVisible()));
    await page.evaluate(() => window.prepareCodeEfforts(['low', 'high', 'max']));
    await page.locator('#refresh-clients').click();
    await codeEffort.waitFor();
    assert.deepEqual(await codeEffort.locator('option').evaluateAll(opts => opts.map(opt => opt.value)), ['', 'low', 'high', 'max']);
    await codeEffort.selectOption('max');
    await launchCode.click();
    await page.getByText('已请求启动 Claude Code。', { exact: true }).waitFor();
    assert.equal(await page.evaluate(() => window.lastLaunchEffort), 'max');
    assert.equal(await page.evaluate(() => window.lastLaunchCwd), codeProject);

    await client('Codex').click();
    assert.ok(!(await projectPanel.isVisible()));
    await page.getByRole('button', { name: '打开 Codex', exact: true }).click();
    assert.deepEqual(await page.evaluate(() => window.launches.at(-1)), ['codex', 'open']);
    assert.ok(!(await page.locator('#terminal-client').isVisible()));
    await client('Codex CLI').click();
    assert.equal(await client('Codex').getAttribute('aria-pressed'), 'false');
    assert.equal(await client('Codex CLI').getAttribute('aria-pressed'), 'true');
    const launchCodex = page.getByRole('button', { name: '启动 Codex CLI', exact: true });
    const applyAndLaunch = page.getByRole('button', { name: '应用并启动 Codex CLI', exact: true });
    await projectPanel.getByText('尚未选择', { exact: true }).waitFor();
    assert.ok(await launchCodex.isDisabled());
    assert.ok(await applyAndLaunch.isDisabled());
    assert.ok(await page.getByRole('button', { name: '应用模式', exact: true }).isEnabled());
    const codexLaunchesBeforePicker = await page.evaluate(() => window.launches.length);
    const switchesBeforePicker = await page.evaluate(() => window.switches.length);
    const pickerCallsBeforeCancel = await page.evaluate(() => window.pickTerminalDirectoryCalls);
    await page.evaluate(() => {
      window.pendingTerminalDirectory = new Promise(resolve => { window.resolveTerminalDirectory = resolve; });
    });
    await chooseProject.click();
    assert.ok(await chooseProject.isDisabled());
    await page.evaluate(() => {
      window.resolveTerminalDirectory(null);
      window.pendingTerminalDirectory = null;
    });
    await page.locator('#choose-terminal-project:enabled').waitFor();
    assert.ok(await applyAndLaunch.isDisabled());
    assert.equal(await page.evaluate(() => window.launches.length), codexLaunchesBeforePicker);
    assert.equal(await page.evaluate(() => window.switches.length), switchesBeforePicker);
    assert.equal(await page.evaluate(() => window.pickTerminalDirectoryCalls), pickerCallsBeforeCancel + 1);
    const codexProject = '/mock/Codex CLI/project';
    await page.evaluate(directory => { window.terminalDirectoryToPick = directory; }, codexProject);
    await chooseProject.click();
    await projectPanel.getByText(codexProject, { exact: true }).waitFor();
    const pickerCallsBeforeLaunch = await page.evaluate(() => window.pickTerminalDirectoryCalls);
    await launchCodex.click();
    await page.getByText('已请求启动 Codex。', { exact: true }).waitFor();
    assert.deepEqual(await page.evaluate(() => window.launches.at(-1)), ['codex', 'terminal']);
    assert.equal(await page.evaluate(() => window.lastLaunchCwd), codexProject);
    assert.equal(await page.evaluate(() => window.pickTerminalDirectoryCalls), pickerCallsBeforeLaunch);
    await page.getByRole('button', { name: '配置新模型' }).click();
    assert.equal(await page.locator('#provider-picker option[value="kimi"]').count(), 0);
    await page.locator('#provider-picker').selectOption('openai');
    await key.fill('fixture-openai-key');
    await page.getByRole('button', { name: '手动选择模型' }).click();
    await page.getByRole('checkbox', { name: 'gpt-6.1-sol', exact: true }).check();
    await page.getByRole('button', { name: '保存档案' }).click();
    await page.getByText('档案已保存。选择连接方式后，点击「应用模式」。').waitFor();
    assert.equal(await page.locator('#client-connection').inputValue(), 'direct');
    await page.locator('#client-connection').selectOption('gateway');
    await page.getByRole('button', { name: '拉取模型', exact: true }).click();
    await page.getByText('供应商模型目录（1 项）', { exact: true }).waitFor();
    await page.getByRole('button', { name: '配置所选模型', exact: true }).click();
    assert.equal(await defaultId(), 'fixture-codex-alt');
    assert.equal(await page.locator('#account-picker').inputValue(), 'openai-account');
    await page.getByRole('button', { name: '保存档案', exact: true }).click();
    await page.getByText('档案已保存。选择连接方式后，点击「应用模式」。').waitFor();
    await page.locator('#client-model-picker').selectOption('gpt-6.1-sol-profile');
    await page.locator('#client-connection').selectOption('gateway');
    page.once('dialog', dialog => dialog.accept());
    await page.getByRole('button', { name: '验证供应商', exact: true }).click();
    await page.getByText('模型调用通过，耗时 23 毫秒。', { exact: true }).waitFor();
    await page.getByRole('heading', { name: 'Codex CLI', exact: true }).waitFor();
    await page.evaluate(() => { window.terminalDirectoryToPick = null; });
    await chooseProject.click();
    await page.locator('#choose-terminal-project:enabled').waitFor();
    assert.ok(await projectPanel.getByText(codexProject, { exact: true }).isVisible());
    const longProject = "/mock/Codex CLI/已选项目 '路径'/" + 'research-project-'.repeat(8);
    await page.evaluate(directory => { window.terminalDirectoryToPick = directory; }, longProject);
    await chooseProject.click();
    await projectPanel.getByText(longProject, { exact: true }).waitFor();
    await applyAndLaunch.click();
    await page.getByText('Codex 配置已写入。', { exact: true }).waitFor();
    assert.deepEqual(await page.evaluate(() => window.switches.at(-1)),
      { client: 'codex', id: 'gpt-6.1-sol-profile', connection: 'gateway', action: 'terminal', cwd: longProject, surface: 'cli' });
    assert.equal(await page.evaluate(() => window.lastSwitchCwd), longProject);
    await gatewayIndicator(true).waitFor();
    await capture('08-codex-switch');
    await client('Codex').click();
    assert.equal(await page.locator('#current-model').innerText(), 'gpt-6.1-sol');
    await client('Claude Code').click();
    assert.ok(await projectPanel.getByText(codeProject, { exact: true }).isVisible());
    await client('Codex CLI').click();
    const codexLaunches = await page.evaluate(() => window.launches.length);
    await page.locator('#client-model-picker').selectOption('fixture-codex-alt-profile');
    await page.locator('#apply-model.primary').click();
    await page.getByText('Codex 网关已切换模型。', { exact: true }).waitFor();
    assert.equal(await page.locator('#current-model').innerText(), 'fixture-codex-alt');
    assert.deepEqual(await page.evaluate(() => window.switches.at(-1)),
      { client: 'codex', id: 'fixture-codex-alt-profile', connection: 'gateway', action: null, surface: 'cli' });
    assert.equal(await page.evaluate(() => window.launches.length), codexLaunches);
    assert.equal(await page.locator('#client-after-apply').isVisible(), false);
    await page.getByLabel('运行模式').selectOption('official');
    await page.getByRole('button', { name: '应用模式', exact: true }).click();
    await page.getByText('Codex 已切回官方模式。', { exact: true }).waitFor();
    await gatewayIndicator(false).waitFor();
    assert.equal(await page.locator('#client-connection').isVisible(), false);
    await client('WorkBuddy').click();
    assert.ok(await page.getByText('未找到可启动的 WorkBuddy。安装后刷新状态。', { exact: true }).isVisible());
    await page.locator('#client-model-picker').selectOption('gpt-6.1-sol-profile');
    await page.getByRole('button', { name: '添加到 WorkBuddy' }).click();
    await page.getByText('已加入 WorkBuddy 自定义模型。').waitFor();

    await page.getByRole('button', { name: '管理 MCP' }).click();
    assert.equal(await page.getByLabel('目标客户端').inputValue(), 'workbuddy');
    await page.getByLabel('目标客户端').selectOption('codex');
    await page.getByLabel('配置范围').waitFor();
    const catalogSearch = page.getByRole('searchbox', { name: '搜索热门 MCP' });
    const catalogCard = page.getByRole('heading', { name: 'Context7', exact: true });
    const frameLeap = page.locator('#mcp-catalog > #frameleap-setup:first-child');
    await catalogCard.waitFor();
    assert.ok(await frameLeap.getByText('HOT', { exact: true }).isVisible());
    await frameLeap.locator('summary').click();
    await page.getByLabel('FrameLeap MCP Key', { exact: true }).waitFor();
    assert.equal(await page.getByLabel('目标客户端').inputValue(), 'desktop');
    await page.getByLabel('目标客户端').selectOption('codex');
    await catalogSearch.fill('无匹配');
    await catalogCard.waitFor({ state: 'hidden' });
    assert.equal(await frameLeap.isVisible(), false);
    await catalogSearch.fill('');
    await catalogCard.waitFor();
    await capture('05-mcp-catalog');
    await page.getByRole('button', { name: '快速配置 Notion', exact: true }).click();
    await page.getByText('首次使用时，运行 codex mcp login notion 完成登录。', { exact: true }).waitFor();
    await page.locator('#close-mcp-quick').click();
    await page.getByLabel('配置范围').selectOption('project');
    await page.getByText('请选择项目根目录。', { exact: true }).waitFor();
    assert.equal(await page.getByRole('button', { name: '快速配置 Context7', exact: true }).isDisabled(), true);
    await page.getByRole('button', { name: '选择项目目录' }).click();
    await page.getByText('/fixture/Project With Spaces', { exact: true }).waitFor();
    await page.getByText('项目共享 · 配置文件', { exact: true }).click();
    assert.ok(await page.getByText('/fixture/Project With Spaces/.codex/config.toml', { exact: true }).isVisible());
    await page.getByRole('button', { name: '快速配置 Context7', exact: true }).click();
    assert.ok(await page.getByText('https://mcp.context7.com/mcp', { exact: true }).isVisible());
    await page.getByLabel('Context7 API Key 的环境变量名（可选）').fill('CONTEXT7_MCP_TOKEN');
    await page.getByRole('button', { name: '保存到客户端' }).click();
    await page.getByText('已保存 context7 到 Codex。').waitFor();
    assert.deepEqual(await page.evaluate(() => {
      const { destination, options, advanced } = window.mcpWrites.at(-1);
      return [destination, options.Authorization, advanced.bearer_token_env_var, window.mcpChecks];
    }), ['codex/project//fixture/Project With Spaces', undefined, 'CONTEXT7_MCP_TOKEN', 0]);
    await page.getByRole('button', { name: '撤销上次 MCP 变更' }).click();
    await page.getByText('已撤销上次 MCP 变更。').waitFor();
    await page.getByText('0 个服务', { exact: true }).waitFor();
    await page.getByLabel('配置范围').selectOption('user');

    await page.getByRole('button', { name: '粘贴配置' }).click();
    await page.getByLabel('配置片段').fill('{"mcpServers":{"custom-service":{"url":"https://example.invalid/custom","trust":true}}}');
    await page.getByRole('button', { name: '读取配置' }).click();
    await page.getByText('以下字段没有导入：trust。请在目标客户端中核对。').waitFor();
    assert.equal(await page.getByLabel('服务名称').inputValue(), 'custom-service');
    assert.equal(await page.getByLabel('MCP 服务地址').inputValue(), 'https://example.invalid/custom');
    assert.equal(await page.locator('#mcp-import-text').inputValue(), '');
    await page.getByText('Codex 高级设置', { exact: true }).click();
    await page.getByLabel('Bearer Token 的环境变量名').fill('CUSTOM_MCP_TOKEN');
    await page.getByLabel('启动超时（秒）').fill('12.5');
    assert.equal(await page.getByLabel('工作目录', { exact: true }).isVisible(), false);
    await page.getByRole('button', { name: '保存 MCP' }).click();
    await page.getByText('已保存 custom-service 到 Codex。').waitFor();
    assert.deepEqual(await page.evaluate(() => window.mcpWrites.at(-1).advanced),
      { bearer_token_env_var: 'CUSTOM_MCP_TOKEN', startup_timeout_sec: 12.5 });
    await page.getByRole('button', { name: '撤销上次 MCP 变更' }).click();
    await page.getByText('0 个服务', { exact: true }).waitFor();
    for (const name of ['docs', 'research']) {
      await page.getByRole('button', { name: '添加 MCP' }).click();
      await page.getByLabel('服务名称').fill(name);
      await page.getByLabel('MCP 服务地址').fill(name === 'docs' ? 'https://example.invalid/mcp' : 'https://example.invalid/denied');
      if (name === 'docs') {
        await page.getByText('请求头（可选）', { exact: true }).click();
        await page.getByRole('button', { name: '添加请求头' }).click();
        await page.getByLabel('请求头名称').fill('Authorization');
        await page.getByLabel('值', { exact: true }).fill('Bearer fixture-secret');
        await page.locator('#verify-mcp').click();
        await page.getByText('验证通过 · 发现 3 个工具', { exact: true }).waitFor();
        assert.equal(await page.getByText('docs', { exact: true }).count(), 0);
      }
      await page.getByRole('button', { name: '保存 MCP' }).click();
      await page.getByText('已保存 ' + name + ' 到 Codex。').waitFor();
    }
    assert.equal(await catalogSearch.isVisible(), false);
    await page.getByText('2 个服务', { exact: true }).waitFor();
    const mcpRow = name => page.locator('.mcp-row').filter({ has: page.getByText(name, { exact: true }) });
    const docsRow = mcpRow('docs');
    const researchRow = mcpRow('research');
    await docsRow.getByText('未验证', { exact: true }).waitFor();
    await Promise.all([
      docsRow.getByText('验证中', { exact: true }).waitFor(),
      docsRow.getByRole('button', { name: '验证连接 docs', exact: true }).click(),
    ]);
    await docsRow.getByText('验证通过', { exact: true }).waitFor();
    assert.ok(await docsRow.getByText('发现 3 个工具', { exact: true }).isVisible());
    assert.ok(await researchRow.getByText('未验证', { exact: true }).isVisible());
    await page.getByLabel('搜索 MCP 服务').fill('research');
    await researchRow.getByRole('button', { name: '验证连接 research', exact: true }).click();
    await researchRow.getByText('验证失败', { exact: true }).waitFor();
    assert.ok(await researchRow.getByText('MCP 认证失败（HTTP 401），请检查凭据或服务权限', { exact: true }).isVisible());
    await page.getByLabel('搜索 MCP 服务').fill('');
    await page.getByRole('button', { name: '刷新列表' }).click();
    await docsRow.getByText('验证通过', { exact: true }).waitFor();
    await researchRow.getByText('验证失败', { exact: true }).waitFor();
    await capture('05-mcp-status');
    await page.getByRole('button', { name: '编辑 docs', exact: true }).click();
    const endpoint = page.getByLabel('MCP 服务地址');
    await page.waitForFunction(() => document.querySelector('#mcp-endpoint').value === 'https://example.invalid/mcp');
    assert.equal(await page.getByLabel('服务名称').getAttribute('readonly'), '');
    await page.getByText('请求头（可选）', { exact: true }).click();
    assert.equal(await page.getByLabel('请求头名称').inputValue(), 'Authorization');
    const credential = page.getByLabel('值', { exact: true });
    assert.equal(await credential.getAttribute('type'), 'password');
    assert.equal(await credential.inputValue(), 'Bearer fixture-secret');
    assert.ok(!(await page.content()).includes('Bearer fixture-secret'));
    await page.getByRole('button', { name: '显示', exact: true }).click();
    assert.equal(await credential.getAttribute('type'), 'text');
    await page.getByRole('button', { name: '隐藏', exact: true }).click();
    await endpoint.fill('https://example.invalid/denied');
    await page.locator('#verify-mcp').click();
    await page.getByText('验证失败。MCP 认证失败（HTTP 401），请检查凭据或服务权限', { exact: true }).waitFor();
    await endpoint.fill('https://example.invalid/updated');
    assert.equal(await page.locator('#mcp-editor-feedback').isVisible(), false);
    await page.getByRole('button', { name: '保存 MCP' }).click();
    await page.getByText('已保存 docs 到 Codex。').waitFor();
    await docsRow.getByText('未验证', { exact: true }).waitFor();
    assert.ok(await researchRow.getByText('验证失败', { exact: true }).isVisible());
    await page.getByRole('button', { name: '编辑 docs', exact: true }).click();
    await page.waitForFunction(() => document.querySelector('#mcp-endpoint').value === 'https://example.invalid/updated');
    await capture('05-mcp-edit');
    await page.setViewportSize({ width: 390, height: 620 });
    await page.getByText('请求头（可选）', { exact: true }).click();
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth));
    await page.setViewportSize({ width: 1080, height: 760 });
    await page.locator('#close-mcp').click();
    await page.getByLabel('搜索 MCP 服务').fill('research');
    await page.getByText('1 / 2 个服务', { exact: true }).waitFor();
    await page.getByRole('button', { name: '停用', exact: true }).click();
    await page.getByText('Streamable HTTP · 配置已停用', { exact: true }).waitFor();
    assert.ok(await researchRow.getByText('未验证', { exact: true }).isVisible());
    await page.getByRole('button', { name: '启用', exact: true }).click();
    await page.getByText('Streamable HTTP · 已写入本机', { exact: true }).waitFor();
    await page.getByLabel('搜索 MCP 服务').fill('');
    await capture('05-mcp');
    page.once('dialog', dialog => dialog.accept());
    await page.getByRole('button', { name: '移除 docs', exact: true }).click();
    await page.getByText('1 个服务', { exact: true }).waitFor();
    await page.getByLabel('目标客户端').selectOption('desktop');
    await page.getByRole('button', { name: '热门 MCP', exact: true }).click();
    await page.getByRole('button', { name: '快速配置 Context7', exact: true }).click();
    await page.getByText('npx -y @upstash/context7-mcp', { exact: true }).waitFor();
    await page.locator('#close-mcp-quick').click();
    await page.locator('#mcp-show-configured').click();
    await page.getByRole('button', { name: '更新 Key' }).waitFor();
    assert.equal(await page.getByText('research', { exact: true }).count(), 0);
    const toolPermissions = page.getByRole('button', { name: '工具权限 frameleap', exact: true });
    await toolPermissions.click();
    const permit = page.getByRole('dialog', { name: '工具权限 · frameleap' });
    const savePermission = permit.getByRole('button', { name: '保存权限' });
    assert.ok(await savePermission.isDisabled());
    await permit.getByRole('button', { name: '读取工具列表' }).click();
    const listProjects = permit.getByRole('checkbox', { name: 'list_projects' });
    await listProjects.waitFor();
    assert.ok(!(await listProjects.isChecked()));
    await listProjects.check();
    await savePermission.click();
    await mcpRow('frameleap').getByText(/始终允许 1 项/).waitFor();
    assert.deepEqual(await page.evaluate(() => window.permissionWrites), [['list_projects']]);
    await toolPermissions.click();
    assert.ok(await listProjects.isChecked());
    await permit.getByRole('button', { name: '清空授权' }).click();
    await savePermission.click();
    await page.getByRole('button', { name: '更新 Key' }).waitFor();
    assert.deepEqual(await page.evaluate(() => window.permissionWrites), [['list_projects'], []]);
    await page.getByRole('button', { name: '前往客户端启动' }).click();
    await page.getByRole('heading', { name: 'Claude Desktop', exact: true }).waitFor();

    await nav('用量').click();
    await page.locator('#usage-total').getByText('0.003 M', { exact: true }).waitFor();
    assert.equal(await page.locator('#usage-total').getAttribute('title'), '2,630 Token');
    assert.equal(await page.locator('#usage-output').innerText(), '< 0.001 M');
    assert.equal(await page.locator('#usage-cache-write').getAttribute('title'), '50 Token');
    assert.ok(await page.getByRole('link', { name: 'OpenAI 官方价' }).isVisible());
    const qwen4Price = page.getByRole('row', { name: /qwen4-max/ }).getByRole('link', { name: '百炼官方价 · 北京' });
    assert.ok(await qwen4Price.isVisible());
    assert.equal(await qwen4Price.getAttribute('href'), 'https://help.aliyun.com/zh/model-studio/model-pricing');
    await page.locator('.usage-pricing summary').click();
    assert.ok(await page.getByRole('link', { name: /1 美元 = 6.7174 元人民币/ }).isVisible());
    await page.locator('.usage-pricing summary').click();
    assert.match(await page.locator('#usage-cost').innerText(), /^< [¥￥]1$/);
    assert.ok(await page.getByRole('link', { name: '智谱官方价' }).isVisible());
    assert.equal(await page.getByRole('link', { name: '智谱官方价' }).getAttribute('title'), '核对于 2026-11-16');
    assert.equal(await page.locator('#usage-cost-caption').innerText(), '已估算 100% Token 的参考费用');
    assert.equal(await page.locator('#usage-pricing-date').innerText(), 'CodeNeo Lite 0.1.8 · 价格表更新于 2026-11-17');
    assert.equal(await page.locator('#usage-cost-note').isVisible(), false);
    assert.equal(await page.locator('#usage-partial').isVisible(), false);
    await page.locator('#usage-client').selectOption('workbuddy');
    await page.getByText('此客户端暂无可读取的本机 Token 明细。', { exact: true }).waitFor();
    assert.equal(await page.locator('#usage-total').innerText(), '—');
    assert.equal(await page.locator('#usage-total').getAttribute('title'), null);
    await page.locator('#usage-client').selectOption('');
    await page.locator('#usage-days').selectOption('1');
    await page.getByText('所选时间内未找到可统计的本机用量。', { exact: true }).waitFor();
    assert.equal(await page.locator('#usage-total').innerText(), '0 M');

    await page.locator('#usage-days').selectOption('30');
    await page.locator('#usage-total').getByText('1,762.99 M', { exact: true }).waitFor();
    assert.equal(await page.locator('#usage-input').innerText(), '1,757.22 M');
    assert.equal(await page.locator('#usage-output').innerText(), '5.77 M');
    assert.match(await page.locator('#usage-cost').innerText(), /^约 [¥￥]2,713$/);
    assert.equal(await page.locator('#usage-cost-label').innerText(), '已估算小计 · 人民币');
    assert.equal(await page.locator('#usage-cost-note').innerText(), '1 项套餐计费 · 1 项实际模型待确认');
    assert.match(await page.locator('#usage-cost-caption').innerText(), /^已估算 94.9% Token/);
    assert.ok(await page.getByRole('link', { name: 'Token Plan 官方说明' }).isVisible());
    assert.ok(await page.getByText('仅支持 Token Plan，暂无按量 API 单价。', { exact: true }).isVisible());
    assert.equal(await page.locator('#usage-partial').innerText(), '部分统计：Codex 的本机用量记录未读全。');
    const topModel = page.locator('#usage-rows tr').first();
    assert.equal(await topModel.locator('.usage-row-total .usage-token').innerText(), '1,413.55 M');
    assert.equal(await topModel.getByText('1,390.06 M', { exact: true }).getAttribute('title'), '1,390,062,208 Token');
    assert.match(await topModel.locator('.usage-row-price').innerText(), /^约 [¥￥]2,203$/);
    assert.match(await page.locator('#usage-rows tr').nth(1).locator('.usage-row-price').innerText(), /^约 [¥￥]498$/);
    await page.mouse.move(4, 4);
    await capture('06-usage', { fullPage: true });
    await page.locator('#usage-client').selectOption('codex');
    assert.equal(await page.locator('#usage-total').innerText(), '1,448.01 M');
    assert.equal(await page.locator('#usage-rows tr').count(), 2);
    assert.match(await page.locator('#usage-cost').innerText(), /^约 [¥￥]2,203$/);
    assert.ok(await page.locator('#usage-partial').isVisible());
    await page.locator('#usage-client').selectOption('desktop');
    assert.equal(await page.locator('#usage-partial').isVisible(), false);
    assert.equal(await page.locator('#usage-cost-label').innerText(), '公开 API 估价 · 人民币');
    assert.equal(await page.locator('#usage-cost-note').isVisible(), false);
    await page.locator('#usage-client').selectOption('');

    // Unknown future models and partly priced rows retain their tokens and cannot
    // turn the subtotal into an apparently complete estimate.
    await page.evaluate(() => { window.usageRows = [
      { client: 'desktop', model: 'glm-5.3', tokens: { input: 1000, output: 0, cache_read: 0, cache_write: 0, cache_write_long: 0 },
        estimate: { low_cny: 0.004, high_cny: 0.004, source: 'glm' }, priced_tokens: 500,
        unpriced: [{ reason: 'unknown_provider', source: null, tokens: 500 }] },
      { client: 'desktop', model: 'future-model', tokens: { input: 1000, output: 0, cache_read: 0, cache_write: 0, cache_write_long: 0 },
        estimate: null, priced_tokens: 0, unpriced: [{ reason: 'missing_price', source: null, tokens: 1000 }] },
    ]; });
    await page.locator('#refresh-usage').click();
    await page.getByText('价格待补充', { exact: true }).waitFor();
    assert.equal(await page.locator('#usage-total').getAttribute('title'), '2,000 Token');
    assert.equal(await page.locator('#usage-cost-caption').innerText(), '已估算 25% Token 的参考费用，其余见明细说明');
    assert.equal(await page.locator('#usage-cost-note').innerText(), '1 项服务商待确认 · 1 项价格待补充');
    assert.ok(await page.getByText('已估算部分用量', { exact: true }).isVisible());
    await page.evaluate(() => { window.usageRows.shift(); });
    await page.locator('#refresh-usage').click();
    await page.getByText('已估算 0% Token 的参考费用，其余见明细说明', { exact: true }).waitFor();
    assert.equal(await page.locator('#usage-cost').innerText(), '—');
    await page.evaluate(() => { delete window.usageRows; });
    await page.locator('#refresh-usage').click();
    await page.locator('#usage-total').getByText('1,762.99 M', { exact: true }).waitFor();

    await page.evaluate(() => window.prepareAgentModels());
    await nav('客户端').click();
    await page.locator('#refresh-clients').click();
    await client('OpenCode').click();
    await page.getByRole('checkbox', { name: /Fixture A/ }).check();
    await page.getByRole('button', { name: '应用模型列表' }).click();
    await page.getByText('已应用模型列表。', { exact: true }).waitFor();
    assert.equal(await page.locator('#current-model').innerText(), 'qa');
    assert.ok(!(await page.locator('#client-after-apply').isVisible()));
    await client('Qwen Code').click();
    await page.getByRole('checkbox', { name: /Fixture A/ }).check();
    await page.getByRole('checkbox', { name: /Fixture B/ }).check();
    await page.locator('.agent-model-row').filter({ hasText: 'Fixture B' })
      .getByRole('button', { name: '设为默认' }).click();
    await page.getByRole('button', { name: '应用模型列表' }).click();
    await page.getByText('已应用模型列表。', { exact: true }).waitFor();
    assert.deepEqual(await page.evaluate(() => window.appliedAgentModels), ['qb', 'qa']);
    assert.equal(await page.locator('#current-model').innerText(), 'qb');
    await client('Claude Desktop').click();
    assert.equal(await page.locator('#agent-models').isVisible(), false);
    await client('Qwen Code').click();
    assert.equal(await page.getByRole('checkbox', { name: /Fixture A/ }).isChecked(), true);

    await page.evaluate(() => window.prepareLongModel());
    await nav('客户端').click();
    await page.locator('#refresh-clients').click();
    for (const size of [{ width: 940, height: 720 }, { width: 790, height: 700 }, { width: 390, height: 640 }]) {
      await page.setViewportSize(size);
      for (const name of ['客户端', '模型库', 'MCP 工具', '用量']) {
        await nav(name).click();
        if (name === '模型库') {
          await catalogTab.click();
          if (size.width === 390) {
            assert.equal(await providerFilter('OpenRouter').isVisible(), false);
            await libraryProvider.selectOption('openrouter');
            assert.equal(await page.locator('.provider-panel').count(), 1);
            const longModel = provider('OpenRouter').locator('.provider-model').last();
            assert.ok(await longModel.locator('code').evaluate(code => code.scrollWidth === code.clientWidth || code.scrollWidth <= Math.ceil(code.getBoundingClientRect().width)));
            await longModel.getByRole('button', { name: /^查看已配置模型 fixture/ }).click();
            await page.getByRole('heading', { name: '长名称模型' }).waitFor();
            assert.equal(await page.evaluate(() => document.documentElement.scrollWidth), size.width);
            await catalogTab.click();
            assert.equal(await libraryProvider.inputValue(), 'openrouter');
            await page.getByRole('button', { name: '清除筛选' }).click();
            await capture('10-library-narrow');
          }
        }
        assert.equal(await page.evaluate(() => document.documentElement.scrollWidth), size.width);
      }
      const tableFits = await page.locator('#usage-table').evaluate(table => table.scrollWidth === table.clientWidth);
      assert.ok(tableFits, `Usage detail should fit at ${size.width}px`);
      if (size.width === 390) {
        assert.ok(await topModel.locator('.usage-cell-label').getByText('缓存读').isVisible());
        await page.mouse.move(4, 4);
        await capture('07-usage-narrow', { fullPage: true });
      }
    }
    await nav('客户端').click();
    await client('Codex CLI').click();
    await page.getByLabel('运行模式').selectOption('third_party');
    await page.locator('#client-model-picker').selectOption('gpt-6.1-sol-profile');
    await page.locator('#client-connection').selectOption('gateway');
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth), 390);
    assert.ok(await page.getByRole('button', { name: '应用并启动 Codex CLI', exact: true }).isEnabled());
    await capture('09-switch-narrow', { fullPage: true });

    await page.setViewportSize({ width: 1080, height: 760 });
    await client('WorkBuddy').click();
    await page.getByRole('button', { name: '配置新模型', exact: true }).click();
    await page.locator('#provider-picker').selectOption('deepseek');
    await key.fill('fixture-deepseek-key');
    await page.getByRole('button', { name: '加载模型列表' }).click();
    await page.getByText('已读取 153 个模型。', { exact: true }).waitFor();
    await candidateSearch.fill('fixture-model-150');
    assert.deepEqual(await candidateIds(), ['fixture-model-150']);
    await candidateSearch.fill('');
    const flash = page.getByRole('checkbox', { name: /DeepSeek Flash/ });
    const pro = page.getByRole('checkbox', { name: /DeepSeek Pro/ });
    await flash.check();
    await pro.check();
    await flash.uncheck();
    assert.equal(await defaultId(), 'deepseek-v4-pro');
    await flash.check();
    await page.getByRole('button', { name: '显示更多模型', exact: true }).click();
    assert.equal(await page.getByRole('checkbox', { name: 'Fixture 150 fixture-model-150', exact: true }).count(), 1);
    assert.equal(await pro.isChecked(), true);
    await page.getByRole('button', { name: '保存并添加 2 个模型', exact: true }).click();
    await page.getByText('已向 WorkBuddy 添加 2 个模型。', { exact: true }).waitFor();
    assert.deepEqual(await page.evaluate(() => window.workbuddyModels), ['deepseek-v4-pro-profile', 'deepseek-flash-profile']);
    await nav('模型库').click();
    await libraryProvider.selectOption('deepseek');
    assert.equal(await page.locator('.saved-model').count(), 2);

    await libraryProvider.selectOption('kimi');
    await kimiProfile.getByRole('button', { name: '编辑', exact: true }).click();
    assert.equal(await page.getByRole('checkbox', { name: 'k3.1-pro', exact: true }).isChecked(), true);
    assert.equal(await page.getByRole('checkbox', { name: 'k3', exact: true }).isChecked(), true);
    await candidateSearch.fill('256k');
    await page.getByRole('checkbox', { name: 'k3-256k', exact: true }).check();
    await page.getByRole('button', { name: '拉取账号目录' }).click();
    await page.getByText('已读取 2 个模型。', { exact: true }).waitFor();
    assert.equal(await candidateSearch.inputValue(), '256k');
    assert.deepEqual(await candidateIds(), []);
    assert.deepEqual(await selectedIds(), ['k3.1-pro', 'k3', 'k3-256k']);
    await candidateSearch.fill('');
    const manualModels = page.getByText('手动添加模型 ID', { exact: true });
    await manualModels.click();
    await page.getByLabel('模型 ID（每行一个）').fill('k3[1m]\nfixture-private-model\nfixture-private-model');
    await page.getByRole('button', { name: '加入列表' }).click();
    await manualModels.click();
    assert.deepEqual(await selectedIds(), ['k3.1-pro', 'k3', 'k3-256k', 'fixture-private-model']);
    await page.getByRole('button', { name: '将 k3-256k 设为默认' }).click();
    await page.getByRole('button', { name: '移除 k3', exact: true }).click();
    assert.equal(await page.getByRole('checkbox', { name: 'Kimi K3 k3', exact: true }).isChecked(), false);
    assert.equal(await defaultId(), 'k3-256k');
    assert.equal(await page.getByLabel('档案模型 ID', { exact: true }).inputValue(), 'k3.1-pro');
    await candidateSearch.focus();
    await page.evaluate(() => window.scrollTo(0, 0));
    await capture('11-model-editor');
    await page.setViewportSize({ width: 390, height: 560 });
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth), 390);
    await capture('12-model-editor-narrow', { fullPage: true });
    await page.setViewportSize({ width: 1080, height: 760 });
    await page.getByRole('button', { name: '测试连接' }).click();
    await page.getByText('模型调用通过，耗时 42 毫秒。', { exact: true }).waitFor();
    assert.equal(await page.evaluate(() => window.probedModel), 'k3-256k');
    await page.getByRole('button', { name: '保存档案', exact: true }).click();
    await kimiProfile.getByText('fixture-private-model', { exact: true }).waitFor();
    assert.deepEqual(await page.evaluate(() => window.editedProfile), {
      id: 'k3.1-pro-profile', model: 'k3.1-pro', desktopModels: ['k3-256k', 'k3.1-pro', 'fixture-private-model'],
    });

    await nav('客户端').click();
    await client('Claude Desktop').click();
    await page.getByLabel('运行模式').selectOption('third_party');
    await page.locator('#client-model-picker').selectOption('k3.1-pro-profile');
    await page.getByRole('button', { name: '配置可选模型', exact: true }).click();
    assert.deepEqual(await selectedIds(), ['k3-256k', 'k3.1-pro', 'fixture-private-model']);
    assert.equal(await defaultId(), 'k3-256k');
    await page.mouse.move(4, 4);
    await page.evaluate(() => document.activeElement.blur());
    await capture('13-desktop-models');
    await page.getByRole('checkbox', { name: 'k3-256k', exact: true }).uncheck();
    assert.equal(await defaultId(), 'k3.1-pro');
    await page.getByRole('button', { name: '保存档案', exact: true }).click();
    await page.getByText('档案已保存。选择连接方式后，点击「应用模式」。').waitFor();
    assert.deepEqual(await page.evaluate(() => window.editedProfile), {
      id: 'k3.1-pro-profile', model: 'k3.1-pro', desktopModels: ['k3.1-pro', 'fixture-private-model'],
    });
    await applyAndRestart.click();
    await confirmRestart();
    await restarted.waitFor();
    const desktopLaunches = await page.evaluate(() => window.launches.length);
    await page.locator('#client-model-picker').selectOption('deepseek-v4-pro-profile');
    await page.locator('#apply-model.primary').click();
    await page.getByText('Claude Desktop 网关已切换模型。', { exact: true }).waitFor();
    assert.equal(await page.locator('#current-model').innerText(), 'deepseek-v4-pro');
    assert.deepEqual(await page.evaluate(() => window.switches.at(-1)),
      { client: 'desktop', id: 'deepseek-v4-pro-profile', connection: 'gateway', action: null });
    assert.equal(await page.evaluate(() => window.launches.length), desktopLaunches);
    assert.equal(await page.locator('#client-after-apply').isVisible(), false);

    const removeProfile = async (id, status) => {
      const card = page.locator('.saved-model').filter({ has: page.locator(`[id="saved-model:${id}"]`) });
      await card.getByText(status, { exact: true }).waitFor();
      await card.getByRole('button', { name: '删除', exact: true }).click();
      const dialog = page.locator('#confirm-dialog');
      await dialog.getByText(/等待应用此档案的任务会取消/).waitFor();
      await dialog.getByRole('button', { name: '确认删除', exact: true }).click();
      await card.waitFor({ state: 'detached' });
      await page.getByText('模型档案已删除。', { exact: true }).waitFor();
    };
    await nav('模型库').click();
    await savedTab.click();
    await removeProfile('deepseek-v4-pro-profile', '使用中');
    await nav('客户端').click();
    assert.equal(await page.locator('#current-model').innerText(), 'deepseek-v4-pro');
    const picker = page.locator('#client-model-picker');
    assert.equal(await picker.locator('option[value="deepseek-v4-pro-profile"]').count(), 0);
    await picker.selectOption('k3.1-pro-profile');
    await page.locator('#apply-model.primary').click();
    await page.getByText('Claude Desktop 网关已切换模型。', { exact: true }).waitFor();
    assert.equal(await page.locator('#current-model').innerText(), 'k3.1-pro');

    await page.evaluate(async () => {
      window.queueCodeSwitch('deepseek-flash-profile');
      await lite.refresh();
    });
    await nav('模型库').click();
    await removeProfile('deepseek-flash-profile', '等待应用');
    await nav('客户端').click();
    await client('Claude Code').click();
    assert.equal(await page.locator('#client-pending').isVisible(), false);
    assert.ok(await launchCode.isEnabled());

    await client('Qwen Code').click();
    assert.ok(await page.getByRole('checkbox', { name: /Fixture B/ }).isChecked());
    await nav('模型库').click();
    await removeProfile('qb', '使用中');
    await nav('客户端').click();
    await page.getByRole('button', { name: '应用模型列表' }).click();
    await page.getByText('已应用模型列表。', { exact: true }).waitFor();
    assert.deepEqual(await page.evaluate(() => window.appliedAgentModels), ['qa']);

    assert.deepEqual(errors, []);
    console.log('codeneo-lite UI: model library filters, model setup and undo, native launch, MCP edit and verification, usage, and narrow layout passed.');
  } finally {
    await browser.close();
  }
}

main().catch(error => { console.error(error); process.exitCode = 1; });

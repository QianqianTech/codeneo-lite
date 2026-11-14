const assert = require('node:assert/strict');
const path = require('node:path');
const { pathToFileURL } = require('node:url');
const { chromium } = require('playwright');

async function main() {
  const browser = await chromium.launch({
    headless: true,
    chromiumSandbox: true,
    ...(process.env.PW_EXECUTABLE_PATH ? { executablePath: process.env.PW_EXECUTABLE_PATH } : {}),
  });
  try {
    const page = await browser.newPage({ viewport: { width: 390, height: 620 } });
    const problems = [];
    page.on('pageerror', error => problems.push(error.message));
    await page.addInitScript(() => {
      let count = 0;
      window.__TAURI__ = { core: { invoke: async (command, args) => {
        if (command === 'lite_gateway_ready') return false;
        if (command === 'lite_overview') return {
          clients: [{ id: 'desktop', name: 'Claude Desktop', protocol: 'anthropic', transports: ['stdio'],
            status: { state: 'third_party', account: '本机网关', model: 'fixture', can_undo: false } }],
          presets: [], accounts: [], models: [], in_use: [],
        };
        if (command === 'list_mcp_catalog') return [];
        if (command === 'list_mcp') return {
          location: 'desktop-user', label: '用户级', path: '/fixture/mcp/config',
          can_undo: false, servers: [{ name: 'frameleap', transport: 'stdio', enabled: true }],
        };
        if (command === 'frameleap_status') return { mode: 'gateway', state: 'existing' };
        if (command !== 'configure_frameleap') throw new Error('Unexpected command');
        if (!args.key.startsWith('fl_test')) throw new Error('Unexpected key');
        count += 1;
        await new Promise(resolve => setTimeout(resolve, 250));
        if (count === 1) throw 'FrameLeap MCP Key 无效、已过期或已撤销';
        return { mode: 'gateway', tool_count: 29 };
      } } };
    });
    await page.goto(pathToFileURL(path.join(__dirname, '../ui/index.html')).href);
    await page.getByRole('heading', { name: 'Claude Desktop', exact: true }).waitFor();
    await page.getByRole('button', { name: 'MCP 工具', exact: true }).click();
    await page.getByText('已有配置', { exact: true }).waitFor();
    await page.getByRole('button', { name: /^已配置/ }).click();
    await page.getByRole('button', { name: '更新 Key' }).click();
    assert.ok(await page.getByText('写入 Claude Desktop 的第三方模型模式。接入后完全退出并重新打开客户端。', { exact: true }).isVisible());

    const key = page.getByLabel('FrameLeap MCP Key', { exact: true });
    const submit = page.locator('#configure-button');
    const dummy = 'fl_test000000000000000000000000000000000000000';
    assert.equal(await key.getAttribute('type'), 'password');
    assert.equal(await submit.isDisabled(), true);
    await key.fill('  ' + dummy + '  ');
    await submit.click();
    await page.getByText('正在验证 Key 并读取 FrameLeap 工具列表…', { exact: true }).waitFor();
    assert.equal(await submit.isDisabled(), true);
    await page.getByText('FrameLeap MCP Key 无效、已过期或已撤销', { exact: true }).waitFor();
    assert.equal(await key.inputValue(), '  ' + dummy + '  ');
    assert.equal(await submit.isEnabled(), true);

    await submit.click();
    await page.getByText('已保存配置并验证 29 个工具。', { exact: true }).waitFor();
    assert.equal(await key.inputValue(), '');
    assert.equal(await submit.isDisabled(), true);
    assert.ok(await page.getByText('已接入', { exact: true }).isVisible());
    assert.ok(!(await page.locator('body').innerText()).includes(dummy));
    assert.equal(await page.evaluate(() => document.documentElement.scrollWidth), 390);
    assert.deepEqual(problems, []);
    console.log('FrameLeap Lite UI: detection, validation error, retry, saved state, and secret masking passed.');
  } finally {
    await browser.close();
  }
}

main().catch(error => { console.error(error); process.exitCode = 1; });

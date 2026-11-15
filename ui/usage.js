"use strict";

(() => {
  const { $, invoke, element, option, message, errorText } = lite;
  const numbers = new Intl.NumberFormat("zh-CN");
  const millions = new Intl.NumberFormat("zh-CN", { maximumFractionDigits: 2 });
  const smallMillions = new Intl.NumberFormat("zh-CN", { maximumFractionDigits: 3 });
  const yuan = new Intl.NumberFormat("zh-CN", { style: "currency", currency: "CNY", maximumFractionDigits: 0 });
  const percent = new Intl.NumberFormat("zh-CN", { style: "percent", maximumFractionDigits: 1 });
  const tokenColumns = [
    { id: "uncached", label: "未缓存输入", amount: tokens => tokens.input },
    { id: "cache-read", label: "缓存读", amount: tokens => tokens.cache_read },
    { id: "cache-write", label: "缓存写", amount: tokens => tokens.cache_write + tokens.cache_write_long },
    { id: "output", label: "输出", amount: tokens => tokens.output },
  ];
  const sources = {
    anthropic: ["Anthropic 官方价", "https://platform.claude.com/docs/en/about-claude/pricing"],
    openai: ["OpenAI 官方价", "https://developers.openai.com/api/docs/pricing"],
    deepseek: ["DeepSeek 峰谷价", "https://api-docs.deepseek.com/quick_start/pricing/"],
    qwen_max: ["百炼官方价 · 北京", "https://help.aliyun.com/zh/model-studio/qwen3-8-max"],
    qwen_flash: ["百炼官方价 · 北京", "https://help.aliyun.com/zh/model-studio/qwen3-8-flash"],
    kimi: ["Kimi 官方价", "https://www.kimi.com/resources/kimi-k3-1-pro-pricing"],
  };
  let report = null;
  let reportDays;
  let busy = false;

  function setTokens(node, tokens) {
    if (tokens == null) {
      node.textContent = "—";
      node.removeAttribute("title");
      return;
    }
    const value = tokens / 1_000_000;
    const format = value < 1 ? smallMillions : millions;
    const amount = tokens > 0 && tokens < 1_000 ? "< 0.001" : format.format(value);
    node.replaceChildren(`${amount} `, element("span", "usage-unit", "M"));
    node.title = `${numbers.format(tokens)} Token`;
  }

  function costText({ low_cny, high_cny }) {
    const estimate = (low_cny + high_cny) / 2;
    return estimate > 0 && estimate < 1 ? `< ${yuan.format(1)}` : `约 ${yuan.format(estimate)}`;
  }

  function sourceLink(label, url, source) {
    const link = element("a", "source-link", label);
    link.href = url;
    link.target = "_blank";
    link.rel = "noopener noreferrer";
    if (invoke) link.addEventListener("click", event => {
      event.preventDefault();
      void invoke("open_reference", { source }).catch(error => message($("usage-feedback"), "error", errorText(error)));
    });
    return link;
  }

  function addCost(cell, estimate) {
    if (!estimate) {
      cell.append(element("span", "muted", "未纳入 API 估价"));
      return;
    }
    cell.append(element("strong", "usage-row-price", costText(estimate)));
    const source = sources[estimate.source];
    if (!source) return;
    cell.append(sourceLink(...source, estimate.source));
  }

  function metricCell(label, className) {
    const cell = element("td", className);
    cell.setAttribute("role", "cell");
    const caption = element("span", "usage-cell-label", label);
    caption.setAttribute("aria-hidden", "true");
    cell.append(caption);
    return cell;
  }

  function modelRow(row, total) {
    const tr = element("tr");
    tr.setAttribute("role", "row");
    const label = element("th", "usage-model");
    label.scope = "row";
    label.setAttribute("role", "rowheader");
    label.append(element("strong", "", row.model), element("small", "",
      [lite.client(row.client)?.name || row.client, row.provider].filter(Boolean).join(" · ")));
    tr.append(label);

    const values = tokenColumns.map(column => column.amount(row.tokens));
    const rowTotal = values.reduce((sum, amount) => sum + amount, 0);
    const totalCell = metricCell("总量", "usage-row-total");
    const totalValue = element("strong", "usage-token");
    setTokens(totalValue, rowTotal);
    totalCell.append(totalValue);
    if (total > 0) {
      const share = element("progress");
      share.max = total;
      share.value = rowTotal;
      share.title = `占当前用量的 ${percent.format(rowTotal / total)}`;
      share.setAttribute("aria-label", "占当前用量的比例");
      totalCell.append(share);
    }
    tr.append(totalCell);

    for (const [index, { label }] of tokenColumns.entries()) {
      const cell = metricCell(label);
      const value = element("span", "usage-token");
      setTokens(value, values[index]);
      cell.append(value);
      tr.append(cell);
    }
    const cost = metricCell("API 估价", "usage-row-cost");
    addCost(cost, row.estimate);
    tr.append(cost);
    return tr;
  }

  function render() {
    if (!report) return;
    const client = $("usage-client").value;
    const rows = report.rows.filter(row => !client || row.client === client);
    const coverage = report.coverage.filter(item => !client || item.client === client);
    const clientName = id => lite.client(id)?.name || id;
    const partial = coverage.filter(item => item.state === "partial");
    $("usage-partial").hidden = !partial.length;
    $("usage-partial").textContent = partial.length
      ? `部分统计：${partial.map(item => clientName(item.client)).join("、")} 的本机用量记录未读全。` : "";
    const amounts = tokenColumns.map(column => rows.reduce((sum, row) => sum + column.amount(row.tokens), 0));
    const [uncached, cacheRead, cacheWrite, output] = amounts;
    const input = uncached + cacheRead + cacheWrite;
    const total = input + output;
    const priced = rows.filter(row => row.estimate);
    const unknown = rows.length - priced.length;
    const unavailable = coverage.length > 0 && coverage.every(item => ["unsupported", "native"].includes(item.state));
    const unreadable = !rows.length && partial.length > 0;
    setTokens($("usage-total"), unavailable || unreadable ? null : total);
    setTokens($("usage-input"), unavailable || unreadable ? null : input);
    $("usage-mix").hidden = total === 0;
    for (const [index, { id }] of tokenColumns.entries()) {
      const amount = amounts[index];
      setTokens($(`usage-${id}`), unavailable || unreadable ? null : amount);
      $(`usage-mix-${id}`).style.flexGrow = total ? amount / total : 0;
    }
    const totalCost = priced.reduce((total, { estimate }) => ({
      low_cny: total.low_cny + estimate.low_cny,
      high_cny: total.high_cny + estimate.high_cny,
    }), { low_cny: 0, high_cny: 0 });
    $("usage-cost").textContent = priced.length ? costText(totalCost) : "—";
    $("usage-cost-caption").hidden = !priced.length;
    $("usage-cost-note").hidden = !unknown;
    $("usage-cost-note").textContent = unknown ? `${unknown} 条明细未纳入估价` : "";
    $("usage-pricing-date").textContent = `价格核对于 ${report.pricing_verified}`;
    $("usage-exchange").replaceChildren(sourceLink(
      `汇率基准 ${report.exchange_rate_date} · 1 美元 = ${report.usd_to_cny} 元人民币`, report.exchange_rate_source, "exchange",
    ));

    const models = document.createDocumentFragment();
    for (const row of rows) models.append(modelRow(row, total));
    $("usage-rows").replaceChildren(models);
    $("usage-table").hidden = !rows.length;
    $("usage-empty").hidden = !!rows.length;
    $("usage-empty").textContent = unavailable ? coverage[0]?.detail || "此客户端暂无可读取的本机 Token 明细。"
      : unreadable ? "当前范围内没有可核验的 Token 明细。" : "所选时间内未找到可统计的本机用量。";
    const note = coverage.map(item => {
      if (client && item.detail) return item.detail;
      return item.state === "native" ? `${clientName(item.client)} 的用量可在原客户端查看` : "";
    }).filter(Boolean);
    $("usage-coverage").hidden = !note.length || unavailable;
    $("usage-coverage").textContent = note.length ? note.join("。") + "。" : "";
  }

  async function refresh() {
    if (!invoke || busy) return;
    busy = true;
    const days = Number($("usage-days").value);
    const results = $("usage-results");
    $("refresh-usage").disabled = true;
    $("usage-days").disabled = true;
    if (days !== reportDays) results.hidden = true;
    results.setAttribute("aria-busy", "true");
    message($("usage-feedback"), "pending", "正在读取本机 Token 记录…");
    try {
      report = await invoke("token_usage", { days });
      reportDays = days;
      render();
      results.hidden = false;
      message($("usage-feedback"), "");
    } catch (error) {
      report = null;
      results.hidden = true;
      message($("usage-feedback"), "error", errorText(error));
    } finally {
      busy = false;
      results.removeAttribute("aria-busy");
      $("refresh-usage").disabled = false;
      $("usage-days").disabled = false;
    }
  }

  function syncClients() {
    const select = $("usage-client");
    const current = select.value;
    const clients = lite.data.clients.filter(client => !client.agent || client.launch?.terminal);
    select.replaceChildren(option("全部客户端", ""), ...clients.map(client => option(client.name, client.id)));
    if (clients.some(client => client.id === current)) select.value = current;
    render();
  }

  $("usage-client").addEventListener("change", render);
  $("usage-days").addEventListener("change", refresh);
  $("refresh-usage").addEventListener("click", refresh);
  window.addEventListener("lite:page", event => { if (event.detail === "usage" && !report) void refresh(); });
  window.addEventListener("lite:refresh", syncClients);
  syncClients();
})();

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
  const reasons = {
    missing_price: ["价格待补充", "Token 已统计，当前价格表尚未收录此模型。"],
    plan_only: ["套餐计费", "仅支持 Token Plan，暂无按量 API 单价。"],
    subscription: ["套餐计费", "本次用量按 Token Plan Credits 抵扣，消耗明细以百炼控制台为准。"],
    dynamic_model: ["实际模型待确认", "记录为路由或套餐模型名，无法确定实际模型单价。"],
    unknown_provider: ["服务商待确认", "记录未提供可核验的服务商，暂无法选择单价。"],
    provider_mismatch: ["服务商价格待核对", "现有单价不适用于记录中的服务商。"],
    request_detail_required: ["缺少逐次请求明细", "该模型按请求长度分档，汇总用量无法确定单价。"],
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

  function priceReference(cell, id) {
    const source = report.pricing_sources?.find(item => item.id === id);
    if (!source) return;
    const link = sourceLink(source.label, source.url, source.id);
    link.title = `核对于 ${source.verified}`;
    cell.append(link);
  }

  function unpricedItems(row) {
    return row.unpriced?.length ? row.unpriced : row.estimate ? [] : [{ reason: "missing_price" }];
  }

  function addCost(cell, row) {
    const unpriced = unpricedItems(row);
    if (row.estimate) {
      cell.append(element("strong", "usage-row-price", costText(row.estimate)));
      if (unpriced.length) cell.append(element("small", "muted", "已估算部分用量"));
      priceReference(cell, row.estimate.source);
    }
    for (const item of unpriced) {
      const [label, detail] = reasons[item.reason] || reasons.missing_price;
      const note = element("div", "usage-price-status");
      note.append(element("span", "", label), element("small", "muted", detail));
      priceReference(note, item.source);
      cell.append(note);
    }
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
    addCost(cost, row);
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
    const omitted = rows.filter(row => unpricedItems(row).length);
    const pricedTokens = rows.reduce((sum, row) => sum + (row.priced_tokens
      ?? (row.estimate ? tokenColumns.reduce((total, column) => total + column.amount(row.tokens), 0) : 0)), 0);
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
    $("usage-cost-label").textContent = omitted.length ? "已估算小计 · 人民币" : "公开 API 估价 · 人民币";
    $("usage-cost-caption").hidden = !rows.length;
    // Do not round incomplete coverage up to 100%.
    const coveragePercent = total ? Math.min(pricedTokens / total, omitted.length ? 0.999 : 1) : 0;
    $("usage-cost-caption").textContent = `已估算 ${percent.format(coveragePercent)} Token 的参考费用${omitted.length ? "，其余见明细说明" : ""}`;
    const issues = new Map();
    for (const row of omitted) {
      const labels = new Set(unpricedItems(row).map(item => (reasons[item.reason] || reasons.missing_price)[0]));
      for (const label of labels) {
        issues.set(label, (issues.get(label) || 0) + 1);
      }
    }
    $("usage-cost-note").hidden = !issues.size;
    $("usage-cost-note").textContent = [...issues].map(([label, count]) => `${count} 项${label}`).join(" · ");
    $("usage-pricing-date").textContent = [report.app_version ? `CodeNeo Lite ${report.app_version}` : "",
      `价格表更新于 ${report.pricing_verified}`].filter(Boolean).join(" · ");
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

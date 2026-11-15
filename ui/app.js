"use strict";

const lite = (() => {
  const $ = id => document.getElementById(id);
  const tauri = window.__TAURI__;
  const invoke = tauri?.core?.invoke;
  const nav = Array.from(document.querySelectorAll(".main-nav [data-page]"));
  const clientSearch = $("client-search");
  const clientSearchToggle = $("toggle-client-search");
  const selection = new Map();
  const switchDrafts = new Map();
  const catalogs = new Map();
  const modeClients = new Set(["desktop", "code", "codex"]);
  const gatewayClients = new Set(["desktop", "codex"]);
  const agentDrafts = new Map();
  const running = new Set();
  const clientMessages = new Map();
  const launchMessages = new Map();
  const launchButtons = {
    "open-client": "open",
    "restart-client": "restart",
    "terminal-client": "terminal",
  };
  const clientNotes = {
    desktop: "应用后，使用上方的「重启 Claude Desktop」让配置生效。",
    code: "退出正在运行的 Claude Code 会话后应用。重新启动后用 /model 选择模型。",
    workbuddy: "重新打开 WorkBuddy，在对话的模型菜单中选择已添加的模型。",
  };
  const codeRoles = {
    opus: "Opus", sonnet: "Sonnet", haiku: "Haiku", fable: "Fable",
    fast: "快速模型", subagent: "子 Agent", reasoning: "推理",
  };
  const states = {
    official: "官方配置", lite: "Lite 已配置", third_party: "已有第三方配置",
    managed: "组织管理", unavailable: "需要初始化", ready: "已找到配置",
    uninitialized: "待配置", not_installed: "未检测到安装",
  };
  const protocols = {
    anthropic: "Anthropic Messages", responses: "OpenAI Responses", chat_completions: "Chat Completions",
  };
  let activeClient = "";
  let activeLaunch = "";
  let refreshSequence = 0;
  let checkingGateway = false;

  function element(tag, className, text) {
    const node = document.createElement(tag);
    if (className) node.className = className;
    if (text) node.textContent = text;
    return node;
  }

  function button(text, handler, className) {
    const node = element("button", className, text);
    node.type = "button";
    node.addEventListener("click", handler);
    return node;
  }

  function option(text, value) { return new Option(text, value); }
  function message(node, kind, text = "") {
    node.dataset.kind = kind;
    node.textContent = text;
    node.hidden = !text;
  }

  function errorText(error) {
    return typeof error === "string" ? error : "操作未完成，请重试。";
  }

  function publish(name, detail) {
    window.dispatchEvent(new CustomEvent(`lite:${name}`, { detail }));
  }

  function client(id = activeClient) {
    return api.data.clients.find(client => client.id === id);
  }

  function canApply(client) {
    return !!(invoke && client?.status && !["managed", "unavailable"].includes(client.status.state));
  }

  function statusLabel(target, installed = target.installed) {
    return states[installed === false ? "not_installed" : target.status.state] || "待检查";
  }

  function agentDraft(target) {
    if (!agentDrafts.has(target.id)) {
      agentDrafts.set(target.id, (target.models_config || []).map(item => ({ ...item })));
    }
    return agentDrafts.get(target.id);
  }

  function renderAgentModels(target, available, locked) {
    const panel = $("agent-models");
    panel.hidden = !target.agent || !available.length;
    if (panel.hidden) return;
    const choices = agentDraft(target);
    const list = $("agent-model-choices");
    const focused = document.activeElement?.id;
    list.replaceChildren();
    for (const model of available) {
      const selected = choices.find(entry => entry.id === model.id);
      const row = element("div", "agent-model-row");
      const label = element("label", "agent-model-choice");
      const check = element("input");
      check.type = "checkbox";
      check.id = "agent-choice-" + model.id;
      check.checked = !!selected;
      check.disabled = locked;
      check.addEventListener("change", () => {
        if (check.checked) choices.push({ id: model.id, context: 32768 });
        else choices.splice(choices.findIndex(item => item.id === model.id), 1);
        renderAgentModels(target, available, locked);
      });
      const title = element("span", "");
      title.append(element("strong", "", model.name));
      const account = api.data.accounts.find(account => account.id === model.account);
      title.append(element("small", "muted", [model.model, account?.name].filter(Boolean).join(" · ")));
      label.append(check, title);
      row.append(label);
      if (selected) {
        const current = choices[0] === selected;
        const makeDefault = button(current ? "默认模型" : "设为默认", () => {
          choices.splice(choices.indexOf(selected), 1);
          choices.unshift(selected);
          renderAgentModels(target, available, locked);
        }, "secondary");
        makeDefault.disabled = current || locked;
        row.append(makeDefault);
      }
      if (selected && target.id === "kimi") {
        const contextLabel = element("label", "agent-context muted", "上下文 Token");
        const context = element("input");
        context.type = "number";
        context.min = 1;
        context.max = 10000000;
        context.value = selected.context;
        context.disabled = locked;
        context.addEventListener("input", () => {
          selected.context = Number(context.value);
          $("save-agent-models").disabled = choices.some(item => !validContext(item.context));
        });
        contextLabel.append(context);
        row.append(contextLabel);
      }
      list.append(row);
    }
    $("save-agent-models").disabled = locked || !choices.length
      || choices.some(item => !validContext(item.context));
    if (focused?.startsWith("agent-choice-")) $(focused)?.focus({ preventScroll: true });
  }

  function validContext(tokens) {
    return Number.isInteger(tokens) && tokens >= 1 && tokens <= 10000000;
  }

  function show(page) {
    if (!nav.some(button => button.dataset.page === page)) return;
    const changed = api.page !== page;
    api.page = page;
    for (const button of nav) {
      if (button.dataset.page === page) button.setAttribute("aria-current", "page");
      else button.removeAttribute("aria-current");
    }
    for (const panel of document.querySelectorAll("[data-view]")) panel.hidden = panel.dataset.view !== page;
    if (changed) {
      window.scrollTo(0, 0);
      document.querySelector(`[data-view="${page}"] h1`)?.focus({ preventScroll: true });
      if (page === "clients") void refreshGateway();
    }
    publish("page", page);
  }

  function defaultLaunch(id) {
    if (id !== "codex") return "";
    const launch = client(id)?.launch;
    return !launch?.open && launch?.terminal ? "terminal" : "open";
  }

  function showClient(id, launch = id === activeClient ? activeLaunch : defaultLaunch(id)) {
    if ((id !== activeClient || launch !== activeLaunch) && api.page === "clients") window.scrollTo(0, 0);
    activeClient = id;
    activeLaunch = launch;
    show("clients");
    renderClients();
  }

  function selectProfile(clientId, id) {
    selection.set(clientId, id);
    if (modeClients.has(clientId)) switchDraft(clientId).mode = "third_party";
  }

  function showSavedProfile(clientId, id, text) {
    selectProfile(clientId, id);
    clientMessages.set(clientId, {
      kind: "success",
      text,
    });
    showClient(clientId);
  }

  async function openClientPanel(id) {
    showClient(id);
    await refresh();
    if (api.page === "clients" && activeClient === id) {
      const first = Object.keys(launchButtons).map($).find(node => !node.hidden && !node.disabled);
      first?.focus();
    }
  }

  function renderClientList() {
    const list = $("client-list");
    const query = clientSearch.value.trim().toLowerCase();
    const entries = api.data.clients.flatMap(item => {
      if (item.id !== "codex") return [{ item, name: item.name, launch: "" }];
      return [
        { item, name: item.name, launch: "open" },
        { item, name: "Codex CLI", launch: "terminal" },
      ];
    });
    const matches = entries.filter(({ name }) => name.toLowerCase().includes(query));
    const focused = list.contains(document.activeElement) ? document.activeElement.dataset.client : "";
    list.replaceChildren();
    for (const { item, name, launch } of matches) {
      const row = button("", () => showClient(item.id, launch), "client-row");
      row.dataset.client = launch === "terminal" ? "codex-cli" : item.id;
      const selected = item.id === activeClient && launch === activeLaunch;
      row.setAttribute("aria-pressed", String(selected));
      row.setAttribute("aria-controls", "client-detail");
      const installed = launch ? item.launch?.[launch] : item.installed;
      row.title = `${name} · ${statusLabel(item, installed)}`;
      const icon = element("span", launch === "terminal" || item.agent || item.id === "code" ? "icon icon-terminal" : "icon icon-clients");
      icon.setAttribute("aria-hidden", "true");
      row.append(icon, element("span", "client-name", name));
      if (installed === false) {
        const state = element("span", "client-install-state", "未检测到");
        state.setAttribute("aria-hidden", "true");
        row.append(state);
      }
      list.append(row);
      if (focused === row.dataset.client) row.focus({ preventScroll: true });
    }
    if (!matches.length) list.append(element("p", "muted", query ? "没有找到匹配的客户端。" : "正在读取本机配置…"));
  }

  function setClientSearch(open) {
    $("client-search-field").hidden = !open;
    clientSearchToggle.setAttribute("aria-expanded", String(open));
    if (open) {
      clientSearch.focus();
    } else {
      clientSearch.value = "";
      renderClientList();
      clientSearchToggle.focus();
    }
  }

  function switchDraft(id) {
    if (!switchDrafts.has(id)) switchDrafts.set(id, {});
    return switchDrafts.get(id);
  }

  function renderSwitch(target, available, locked) {
    const { id, status, launch } = target;
    const supportsMode = modeClients.has(id);
    const supportsGateway = gatewayClients.has(id);
    const draft = switchDrafts.get(id);
    const mode = $("client-mode");
    const picker = $("client-model-picker");
    const connection = $("client-connection");
    const selected = selection.get(id) ?? status.selected ?? "";
    const profile = available.find(model => model.id === selected);
    const thirdPartyActive = !!profile || ["third_party", "lite"].includes(status.state) || !!status.pending?.id;
    const third = !supportsMode || (draft?.mode ?? (thirdPartyActive ? "third_party" : "official")) === "third_party";
    const gatewayRequired = id === "desktop" && profile?.desktop_direct === false;
    const defaultsToGateway = status.local_gateway
      || (id === "desktop" && profile?.id !== status.selected && profile?.desktop_models?.length);
    const route = gatewayRequired ? "gateway" : draft?.connection ?? (defaultsToGateway ? "gateway" : "direct");
    $("model-switch").hidden = target.agent || (!supportsMode && !available.length);
    $("client-mode-field").hidden = !supportsMode;
    mode.options[0].textContent = id === "codex" ? "OpenAI 官方" : "Claude 原生";
    mode.value = third ? "third_party" : "official";
    mode.disabled = locked;
    $("client-profile-field").hidden = !third;
    $("client-profile-label").textContent = supportsMode ? "第三方档案" : "从已保存的档案选择";
    picker.replaceChildren(option("请选择已保存的档案", ""));
    for (const model of available) {
      const account = api.data.accounts.find(account => account.id === model.account);
      picker.append(option([model.name, account?.name].filter(Boolean).join(" · "), model.id));
    }
    picker.value = profile?.id || "";
    picker.disabled = locked || !available.length;
    $("client-connection-field").hidden = !supportsGateway || !third;
    connection.options[0].disabled = gatewayRequired;
    connection.value = route;
    connection.disabled = locked;

    const local = supportsGateway && third && route === "gateway";
    const ready = !!profile || !third;
    const hotSwitch = local && status.local_gateway;
    const desktopReload = id === "desktop" && launch?.restart && !hotSwitch;
    const action = id === "desktop" && launch?.open ? (launch.restart ? "restart" : "open")
      : id === "codex" && launch?.[activeLaunch] ? activeLaunch : "";
    const apply = $("apply-model");
    apply.textContent = supportsMode ? "应用模式" : id === "workbuddy" ? "添加到 WorkBuddy" : "应用模型";
    apply.classList.toggle("primary", hotSwitch || !action);
    apply.classList.toggle("secondary", !hotSwitch && !!action);
    apply.disabled = locked || !ready || !!desktopReload;
    const launchButton = $("apply-client-launch");
    launchButton.hidden = !action;
    launchButton.disabled = locked || !ready;
    launchButton.classList.toggle("primary", !hotSwitch);
    launchButton.classList.toggle("secondary", hotSwitch);
    launchButton.dataset.action = action;
    launchButton.textContent = action === "terminal" ? "应用并启动 Codex CLI" : action === "restart" ? "应用并重启" : "应用并打开";

    const edit = $("edit-client-model");
    edit.hidden = !(third && profile && (id === "code" || id === "desktop"));
    edit.textContent = id === "desktop" ? "配置可选模型" : "配置模型映射";
    edit.disabled = locked;
    for (const control of [$("pull-client-models"), $("verify-client-model")]) {
      control.hidden = !supportsMode || !third;
      control.disabled = locked || !profile;
    }
    const note = $("client-mode-note");
    const guidance = local
      ? (hotSwitch
        ? "点击「应用模式」后，原会话的下一条请求会使用所选档案，并将续聊上下文发给该供应商。请保持 Lite 运行。"
        : "首次接入网关后请重新打开客户端，并保持 Lite 运行。")
      : id === "codex" && third && profile
        ? "直连会将此账号下已保存的模型加入 Codex 的模型列表。所选档案是默认模型。" : "";
    note.textContent = [guidance, desktopReload ? "当前窗口正在运行，应用此配置需要重启 Claude Desktop。" : ""].filter(Boolean).join(" ");
    note.hidden = !supportsMode || !note.textContent;
    const catalog = catalogs.get(id);
    const showCatalog = third && profile && catalog?.id === profile.id && catalog.models.length;
    $("client-catalog").hidden = !showCatalog;
    if (showCatalog) {
      const catalogPicker = $("client-catalog-model");
      const chosen = catalogPicker.value;
      catalogPicker.replaceChildren(...catalog.models.map(model => option(model.name && model.name !== model.id ? `${model.name} · ${model.id}` : model.id, model.id)));
      if (catalog.models.some(model => model.id === chosen)) catalogPicker.value = chosen;
      $("client-catalog-title").textContent = `供应商模型目录（${catalog.models.length} 项${catalog.partial ? "，部分目录" : ""}）`;
      catalogPicker.disabled = $("configure-catalog-model").disabled = locked;
    }
    return { supportsMode, third };
  }

  function renderClients() {
    if (!client() && api.data.clients.length) {
      activeClient = api.data.clients[0].id;
      activeLaunch = defaultLaunch(activeClient);
    }
    renderClientList();
    const target = client();
    $("client-identity").hidden = !target;
    $("client-detail").hidden = !target;
    if (!target) return;
    const status = target.status;
    const locked = !canApply(target) || running.has(target.id);
    const workbuddy = target.id === "workbuddy";
    const codexCli = target.id === "codex" && activeLaunch === "terminal";
    const installed = target.id === "codex" ? target.launch?.[activeLaunch] : target.installed;
    $("current-client").textContent = codexCli ? "Codex CLI" : target.name;
    $("client-status").textContent = statusLabel(target, installed);
    $("client-status").dataset.state = installed === false ? "not_installed" : status.state;
    renderLaunch(target);
    $("current-model").textContent = status.state === "unavailable" ? "—" : status.model || "由客户端选择";
    $("current-account").textContent = status.state === "unavailable" ? "—" : status.account || "由客户端配置";
    $("current-model-label").textContent = workbuddy ? "生效方式" : "当前模型";
    $("current-account-label").textContent = workbuddy ? "模型列表" : "接入来源";
    $("switch-title").textContent = target.agent ? "模型配置" : workbuddy ? "添加自定义模型" : "模型切换";
    const applied = $("current-models");
    const entries = workbuddy ? status.models || []
      : (target.id === "desktop" || target.id === "codex" || target.agent) && (status.models?.length || 0) > 1
      ? status.models.map((id, i) => (target.agent || target.id === "codex" ? id === status.model : i === 0) ? `默认 · ${id}` : id)
      : (status.code_models || []).map(({ role, model }) => `${codeRoles[role] || role} · ${model}`);
    applied.hidden = entries.length === 0;
    applied.replaceChildren(...entries.map(id => element("code", "", id)));
    const gatewayStatus = $("gateway-status");
    gatewayStatus.hidden = !status.local_gateway;
    gatewayStatus.textContent = status.gateway_verified ? "CodeNeo Lite网关 · 已完成转发"
      : status.gateway_ready ? "CodeNeo Lite网关 · 已就绪" : "CodeNeo Lite网关";
    const supported = target.protocols || [target.protocol];
    $("client-protocol").textContent = supported.length
      ? "支持 " + supported.map(protocol => protocols[protocol] || protocol).join("、")
      : "在原客户端选择模型";
    message($("client-issue"), status.state === "unavailable" ? "warning" : "info", status.issue);

    const available = api.data.models.filter(model => model.clients.includes(target.id));
    const { supportsMode, third } = renderSwitch(target, available, locked);
    renderAgentModels(target, available, locked);
    $("client-empty-text").hidden = available.length > 0 || (supportsMode && !third);
    $("client-empty-text").textContent = api.data.models.length
      ? "模型库中没有适用于此客户端的模型。"
      : "选择服务商并填写 API Key 后，即可添加模型。";
    $("client-add-model").classList.toggle("primary", available.length === 0);
    $("client-add-model").disabled = !invoke || running.has(target.id);
    const showInstall = target.installed === false && target.install_help;
    const help = $("agent-help");
    help.hidden = !target.help && !showInstall;
    help.dataset.install = String(!!showInstall);
    help.textContent = showInstall ? "查看安装说明 ↗" : "打开官方配置指南 ↗";
    $("client-note").textContent = status.local_gateway && target.id === "desktop"
      ? "重新打开 Claude Desktop 可刷新它的模型菜单。"
      : status.local_gateway && target.id === "codex"
      ? "Codex 桌面端与 CLI 共用网关档案。当前转发的模型在 Lite 中查看。"
      : codexCli ? "与 Codex 桌面端共用用户配置。应用后启动新会话，在 /model 中选择模型。"
      : target.id === "codex" ? "与 Codex CLI 共用用户配置。完全退出并重新打开桌面应用后生效。"
      : target.id === "desktop" && !target.launch?.restart
      ? "应用后，完全退出并重新打开 Claude Desktop。"
      : target.note || clientNotes[target.id] || "";
    const restore = $("restore-model");
    restore.textContent = "恢复原配置";
    restore.hidden = !(target.id === "codex" && status.can_restore || target.agent && status.state === "lite");
    restore.disabled = locked;
    $("undo-model").hidden = !status.can_undo;
    $("undo-model").disabled = locked;
    const recovery = $("recover-desktop-session");
    recovery.hidden = target.id !== "desktop" || !status.recover_session;
    recovery.disabled = locked;
    $("client-recovery").hidden = !canApply(target) || (restore.hidden && !status.can_undo && recovery.hidden);
    $("client-pending").hidden = !status.pending;
    if (status.pending) {
      const model = api.data.models.find(model => model.id === status.pending.id);
      $("pending-text").textContent = status.pending.issue || `等待 Claude Code 会话退出后应用 ${model?.name || "Claude 官方配置"}。`;
    }
    $("cancel-switch").disabled = locked;
    const feedback = clientMessages.get(target.id);
    message($("client-feedback"), feedback?.kind || "", feedback?.text);
    const after = $("client-after-apply");
    const action = target.id === "codex" ? activeLaunch
      : target.launch?.restart ? "restart"
      : target.agent && target.launch?.terminal ? "terminal"
        : target.launch?.open ? "open" : "terminal";
    after.hidden = !feedback?.actionable || !target.launch?.[action] || !!status.pending;
    after.disabled = running.has(target.id);
    after.dataset.action = action;
    after.textContent = action === "restart" ? "重启以生效"
      : action === "open" ? "打开客户端" : "启动新会话";
  }

  function renderLaunch(target) {
    const controls = target.launch;
    const codexAction = target.id === "codex" ? activeLaunch : "";
    const primaryAction = codexAction || (controls?.open ? "open" : "terminal");
    for (const [id, action] of Object.entries(launchButtons)) {
      const node = $(id);
      node.hidden = !controls?.[action] || (!!codexAction && action !== codexAction);
      node.disabled = !invoke || running.has(target.id) || (target.id === "code" && !!target.status.pending);
      node.classList.toggle("primary", action === primaryAction);
      node.classList.toggle("secondary", action !== primaryAction);
    }
    $("client-launch").hidden = Object.keys(launchButtons).every(id => $(id).hidden);
    $("open-client").textContent = `打开 ${target.name}`;
    $("restart-client").textContent = `重启 ${target.name}`;
    $("terminal-client").textContent = target.id === "codex" ? "启动 Codex CLI" : "在终端启动";
    $("launch-note").textContent = codexAction && controls?.[codexAction] === false
      ? (codexAction === "terminal" ? "未检测到 codex 命令。安装后刷新状态。" : "未检测到 Codex 桌面应用。")
      : codexAction === "open" ? ""
      : controls?.note || (controls?.version ? "CLI " + controls.version : "");
    $("launch-note").hidden = !$("launch-note").textContent;
    const feedback = launchMessages.get(target.id);
    message($("launch-feedback"), feedback?.kind || "", feedback?.text);
  }

  async function confirmRestart(name) {
    const dialog = $("restart-dialog");
    if (dialog.open) return false;
    $("restart-title").textContent = `重启 ${name}？`;
    dialog.returnValue = "";
    return new Promise(resolve => {
      dialog.addEventListener("close", () => resolve(dialog.returnValue === "restart"), { once: true });
      dialog.showModal();
    });
  }

  async function startClient(action) {
    const target = client();
    if (!invoke || !target?.launch?.[action] || running.has(target.id)) return;
    if (action === "restart" && !(await confirmRestart(target.name))) return;
    const { id } = target;
    running.add(id);
    launchMessages.set(id, { kind: "pending", text: action === "restart" ? "正在重启客户端…" : "正在打开客户端…" });
    renderClients();
    try {
      const result = await invoke("launch_client", { client: id, action });
      launchMessages.set(id, { kind: "success", text: result });
      const configured = clientMessages.get(id);
      if (configured) configured.actionable = false;
      await refresh();
    } catch (error) {
      launchMessages.set(id, { kind: "error", text: errorText(error) });
    } finally {
      running.delete(id);
      renderClients();
    }
  }

  async function refreshGateway() {
    if (!invoke || checkingGateway) return;
    checkingGateway = true;
    const ready = await invoke("lite_gateway_ready").catch(() => false);
    checkingGateway = false;
    const indicator = $("gateway-indicator");
    const label = ready ? "网关正常" : "网关未就绪";
    indicator.dataset.ready = String(ready);
    indicator.title = label;
    indicator.setAttribute("aria-label", label);
  }

  async function refresh() {
    void refreshGateway();
    const index = ++refreshSequence;
    $("refresh-clients").disabled = true;
    try {
      if (!invoke) throw "请在 CodeNeo Lite 桌面窗口中打开。";
      const view = await invoke("lite_overview");
      if (index !== refreshSequence) return;
      api.data = view;
      message($("workspace-error"), "");
      renderClients();
      publish("refresh", view);
    } catch (error) {
      if (index === refreshSequence) {
        message($("workspace-error"), "error", errorText(error));
        publish("refresh-error");
      }
    } finally {
      if (index === refreshSequence) $("refresh-clients").disabled = false;
    }
  }

  async function clientAction(id, command, args = {}, saved = false) {
    if (!canApply(client(id)) || running.has(id)) return false;
    running.add(id);
    const reopen = command === "switch_model_mode" && !!args.action;
    const hotSwitch = command === "switch_model_mode" && args.connection === "gateway" && client(id).status.local_gateway;
    const pending = reopen ? (args.action === "restart" ? "正在应用配置并重启客户端…" : "正在应用模式并启动客户端…") : "正在写入本机配置…";
    clientMessages.set(id, { kind: "pending", text: pending });
    renderClients();
    try {
      const result = await invoke(command, { client: id, ...args });
      selection.delete(id);
      switchDrafts.delete(id);
      catalogs.delete(id);
      agentDrafts.delete(id);
      clientMessages.set(id, {
        kind: "success",
        text: (typeof result === "string" ? result : result?.message) || "已取消等待中的切换。",
        actionable: !reopen && !hotSwitch && ["apply_model", "apply_agent_models", "undo_model", "switch_model_mode"].includes(command),
      });
      await refresh();
      return true;
    } catch (error) {
      const text = saved ? `模型已保存。应用失败：${errorText(error)}` : errorText(error);
      clientMessages.set(id, { kind: "error", text });
      if (command === "switch_model_mode") await refresh();
      return false;
    } finally {
      running.delete(id);
      renderClients();
    }
  }

  async function applyMode(action = null) {
    const target = client();
    if (!canApply(target)) return;
    const modes = modeClients.has(target.id);
    const third = !modes || $("client-mode").value === "third_party";
    const id = third ? $("client-model-picker").value : null;
    if (third && !id) return;
    const connection = third && gatewayClients.has(target.id) ? $("client-connection").value : null;
    if (action === "restart" && !(await confirmRestart(target.name))) return;
    void clientAction(target.id, modes ? "switch_model_mode" : "apply_model",
      modes ? { id, connection, action } : { id });
  }

  async function inspectSavedModel(check) {
    const target = client();
    const id = $("client-model-picker").value;
    if (!canApply(target) || !id || running.has(target.id)) return;
    if (check && !window.confirm("验证供应商会发送一条简短请求，可能产生少量调用费用。")) return;
    const clientId = target.id;
    const connection = gatewayClients.has(clientId) ? $("client-connection").value : null;
    if (!check) catalogs.delete(clientId);
    running.add(clientId);
    clientMessages.set(clientId, { kind: "pending", text: check ? "正在验证供应商…" : "正在拉取模型目录…" });
    renderClients();
    try {
      const result = await invoke(check ? "verify_saved_model" : "saved_model_catalog", check ? { client: clientId, id, connection } : { client: clientId, id });
      if (check) {
        const text = result.healthy ? `${result.message}，耗时 ${result.latency_ms} 毫秒。` : result.message;
        clientMessages.set(clientId, { kind: result.healthy ? "success" : "error", text });
      } else {
        const models = result.models.filter(model => clientId !== "desktop" || model.id.toLowerCase() !== "auto");
        catalogs.set(clientId, { id, models, partial: result.partial });
        clientMessages.set(clientId, { kind: models.length ? "success" : "info", text: models.length
          ? `已拉取 ${models.length} 个模型。可选择一个加入模型库。`
          : "供应商未返回可用的模型目录，可手动填写模型 ID。" });
      }
    } catch (error) {
      clientMessages.set(clientId, { kind: "error", text: errorText(error) });
    } finally {
      running.delete(clientId);
      renderClients();
    }
  }

  const api = {
    $, invoke, element, button, option, message, errorText, protocols,
    client, canApply, show, showClient, showSavedProfile, selectProfile, openClientPanel, refresh, clientAction, codeRoles,
    page: "clients", data: { clients: [], presets: [], accounts: [], models: [], in_use: [] },
  };

  api.addAgentDraft = (id, models) => {
    const target = client(id);
    if (!target) return;
    const choices = agentDraft(target);
    for (const model of models) {
      if (!choices.some(item => item.id === model)) choices.push({ id: model, context: 32768 });
    }
    showClient(id);
  };

  for (const link of nav) link.addEventListener("click", () => {
    if (api.page === link.dataset.page) window.scrollTo(0, 0);
    show(link.dataset.page);
  });
  for (const [id, action] of Object.entries(launchButtons)) {
    $(id).addEventListener("click", () => startClient(action));
  }
  $("client-after-apply").addEventListener("click", event => startClient(event.currentTarget.dataset.action));
  $("agent-help").addEventListener("click", event => {
    const id = activeClient;
    if (invoke) void invoke("open_agent_help", { client: id, install: event.currentTarget.dataset.install === "true" }).catch(error => {
      clientMessages.set(id, { kind: "error", text: errorText(error) });
      renderClients();
    });
  });
  $("save-agent-models").addEventListener("click", () => {
    const target = client();
    if (target?.agent) void clientAction(target.id, "apply_agent_models", { models: agentDraft(target) });
  });
  clientSearchToggle.addEventListener("click", () => setClientSearch($("client-search-field").hidden));
  clientSearch.addEventListener("input", renderClientList);
  clientSearch.addEventListener("keydown", event => {
    if (event.key === "Escape") {
      event.preventDefault();
      setClientSearch(false);
    }
  });
  $("refresh-clients").addEventListener("click", refresh);
  $("client-add-model").addEventListener("click", event => publish("new-model", { target: activeClient, opener: event.currentTarget }));
  $("edit-client-model").addEventListener("click", event => {
    const model = api.data.models.find(model => model.id === $("client-model-picker").value);
    if (model) publish("new-model", { target: activeClient, model, showOptions: true, opener: event.currentTarget });
  });
  $("client-mcp").addEventListener("click", () => publish("mcp-target", activeClient));
  $("open-model-switch").addEventListener("click", () => {
    showClient(activeClient);
    ($("client-mode-field").hidden ? $("client-model-picker") : $("client-mode")).focus();
  });
  $("client-mode").addEventListener("change", event => {
    switchDraft(activeClient).mode = event.target.value;
    clientMessages.delete(activeClient);
    renderClients();
  });
  $("client-connection").addEventListener("change", event => {
    switchDraft(activeClient).connection = event.target.value;
    clientMessages.delete(activeClient);
    renderClients();
  });
  $("client-model-picker").addEventListener("change", event => {
    selection.set(activeClient, event.target.value);
    clientMessages.delete(activeClient);
    renderClients();
  });
  $("apply-model").addEventListener("click", () => applyMode());
  $("apply-client-launch").addEventListener("click", event => applyMode(event.currentTarget.dataset.action));
  $("pull-client-models").addEventListener("click", () => inspectSavedModel(false));
  $("verify-client-model").addEventListener("click", () => inspectSavedModel(true));
  $("configure-catalog-model").addEventListener("click", event => {
    const profile = api.data.models.find(model => model.id === $("client-model-picker").value);
    const catalog = catalogs.get(activeClient);
    const modelId = $("client-catalog-model").value;
    if (profile && catalog?.id === profile.id && catalog.models.some(model => model.id === modelId)) {
      publish("new-model", { target: activeClient, accountId: profile.account, modelId, candidates: catalog, opener: event.currentTarget });
    }
  });
  $("restore-model").addEventListener("click", () => clientAction(activeClient, "apply_model", { id: null }));
  $("undo-model").addEventListener("click", () => clientAction(activeClient, "undo_model"));
  $("cancel-switch").addEventListener("click", () => clientAction(activeClient, "cancel_code_switch"));
  $("recover-desktop-session").addEventListener("click", () => {
    const request = client("desktop")?.status.recover_session;
    if (request && window.confirm("将 Claude Desktop 最近请求接续的旧会话发送给当前服务商。确认接续？")) {
      void clientAction("desktop", "recover_desktop_model_session", { request });
    }
  });
  if (tauri?.event?.listen) {
    void tauri.event.listen("lite-page", ({ payload }) => {
      if (["models", "tools", "usage"].includes(payload)) show(payload);
      else if (payload === "codex-cli") showClient("codex", "terminal");
      else showClient(payload);
    }).catch(() => {});
    void tauri.event.listen("lite-change", ({ payload }) => {
      clientMessages.set(payload.client, { kind: payload.error ? "error" : "success", text: payload.message });
      selection.delete(payload.client);
      switchDrafts.delete(payload.client);
      catalogs.delete(payload.client);
      agentDrafts.delete(payload.client);
      void refresh();
    }).catch(() => {});
    void tauri.event.listen("lite-launch", ({ payload }) => {
      launchMessages.set(payload.client, { kind: payload.error ? "error" : "success", text: payload.message });
      void refresh();
    }).catch(() => {});
  }
  window.addEventListener("focus", () => { if (invoke) void refresh(); });
  window.addEventListener("DOMContentLoaded", refresh, { once: true });
  if (invoke) {
    window.setInterval(() => {
      if (!document.hidden && api.page === "clients") void refreshGateway();
    }, 4000);
  }
  return api;
})();

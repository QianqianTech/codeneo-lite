"use strict";

(() => {
  const { $, invoke, element, button, option, message, errorText } = lite;
  const target = $("mcp-client");
  const form = $("mcp-editor");
  const quickForm = $("mcp-quick");
  const importForm = $("mcp-import");
  const permissionDialog = $("mcp-permissions-dialog");
  const permissionFeedback = $("mcp-permissions-feedback");
  const frameLeap = $("frameleap-setup");
  const transportLabels = { stdio: "本地命令（stdio）", http: "Streamable HTTP", sse: "HTTP SSE", other: "其他类型" };
  const checkLabels = { pending: "验证中", success: "验证通过", error: "验证失败" };
  const scopeLabels = { user: "用户级", project: "项目共享", local: "本机项目" };
  const notes = {
    desktop: "完全退出并重新打开 Claude Desktop，再到「设置 → Developer」查看工具。",
    code: "新开 Claude Code 会话，输入 /mcp 查看服务。",
    codex: "重开 Codex 或启动新 CLI 进程后，查看 MCP 设置。",
    workbuddy: "在 WorkBuddy 的「连接器 → 自定义连接器」中信任服务，再开启新对话。",
  };
  let selected = "";
  let scope = "user";
  let project = "";
  let location = null;
  let view = "catalog";
  let catalog = [];
  let catalogError = "";
  let catalogSequence = 0;
  let preset = null;
  let editorDestination = null;
  let nativeFields = [];
  let editing = null;
  let revision = null;
  let busy = false;
  let sequence = 0;
  let servers = [];
  let checkLocation = "";
  let checks = new Map();
  let permission = null;
  const originalValues = new WeakMap();
  const editorFeedback = $("mcp-editor-feedback");
  const quickFeedback = $("mcp-quick-feedback");
  const importFeedback = $("mcp-import-feedback");
  const pageFeedback = $("mcp-feedback");
  const targetFeedback = $("mcp-target-feedback");

  function selection() { return { scope, directory: scope === "user" ? null : project || null }; }
  function destination() { return { ...selection(), location: location?.location }; }
  function canConfigure() {
    const client = lite.client(selected);
    return !!(invoke && !busy && location && client?.transports.length);
  }
  function targetLabel() { return [lite.client(selected)?.name, location?.label].filter(Boolean).join(" · "); }

  function controls() {
    target.disabled = busy || !lite.data.clients.length;
    $("mcp-scope").disabled = busy;
    $("pick-mcp-project").disabled = !invoke || busy;
    $("new-mcp").disabled = !canConfigure();
    $("import-mcp").disabled = !canConfigure();
    $("refresh-mcp").disabled = !invoke || busy || !selected;
    $("undo-mcp").hidden = !location?.can_undo;
    $("undo-mcp").disabled = busy;
    const launch = lite.client(selected)?.launch;
    $("mcp-client-settings").hidden = !(launch?.open || launch?.terminal);
    $("mcp-client-settings").disabled = busy;
    for (const input of form.querySelectorAll("input, textarea, select, button")) {
      input.disabled = busy || (input.id !== "close-mcp" && (!location || editing !== null && revision === null));
    }
    for (const node of [importForm, quickForm]) {
      for (const input of node.querySelectorAll("input, textarea, button")) input.disabled = busy;
    }
    for (const action of $("mcp-catalog-list").querySelectorAll("button[data-configure]")) {
      action.disabled = !canConfigure() || action.hasAttribute("data-unavailable");
    }
    $("mcp-name").readOnly = !!editing;
    const list = $("mcp-list");
    const loading = list.getAttribute("aria-busy") === "true";
    $("mcp-search").disabled = busy || loading || !servers.length;
    for (const action of list.querySelectorAll("button")) action.disabled = busy || loading;
    form.setAttribute("aria-busy", String(busy));
  }

  function syncTargets() {
    const clients = lite.data.clients;
    if (!selected || !lite.client(selected)) {
      selected = lite.client()?.id || clients[0]?.id || "";
      scope = "user";
    }
    target.replaceChildren(...clients.map(client => option(client.name, client.id)));
    target.value = selected;
    const scopes = lite.client(selected)?.mcp_scopes || ["user"];
    if (!scopes.includes(scope)) scope = "user";
    $("mcp-scope").replaceChildren(...scopes.map(id => option(scopeLabels[id], id)));
    $("mcp-scope").value = scope;
    $("mcp-scope-field").hidden = scopes.length < 2;
    $("mcp-project").hidden = scope === "user";
    $("mcp-project-path").textContent = project || "请选择项目根目录。";
    $("pick-mcp-project").textContent = project ? "更换项目目录" : "选择项目目录";
    const scopeNote = $("mcp-scope-note");
    scopeNote.hidden = scope === "user";
    scopeNote.textContent = scope === "local" ? "仅在本机的这个项目中生效。" :
      "写入项目目录，可随项目共享。请使用环境变量引用凭据，避免把 Key 提交到仓库。";
    if (selected === "code" && scope !== "user") {
      scopeNote.textContent += " 同名服务按本机项目、项目共享、用户级的顺序优先生效。";
    }
    $("mcp-note").textContent = lite.client(selected)?.mcp_note || notes[selected] || "";
    controls();
  }

  function showView(next) {
    view = next;
    const browse = view === "catalog";
    $("mcp-show-catalog").setAttribute("aria-pressed", String(browse));
    $("mcp-show-configured").setAttribute("aria-pressed", String(!browse));
    $("mcp-catalog-search-field").hidden = !browse;
    $("mcp-catalog").hidden = !browse;
    $("mcp-configured").hidden = browse;
    if (!browse) frameLeap.open = false;
  }

  function openFrameLeap() {
    if (!invoke || busy || !lite.client("desktop")) return;
    closePanels();
    if (selected !== "desktop") {
      selected = "desktop";
      scope = "user";
      resetTarget();
      refresh();
    }
    if (frameLeap.hidden) {
      $("mcp-catalog-search").value = "";
      renderCatalog();
    }
    showView("catalog");
    frameLeap.open = true;
    $("mcp-key").focus();
  }

  function renderLocation() {
    $("mcp-location").hidden = !location;
    $("mcp-location-label").textContent = location ? `${location.label} · 配置文件` : "配置文件";
    $("mcp-location-path").textContent = location?.path || "";
    $("mcp-tab-count").textContent = location ? String(servers.length) : "";
  }

  function showServerCheck(status, feedback, { kind, detail }) {
    status.dataset.state = kind;
    status.textContent = checkLabels[kind];
    message(feedback, kind, detail);
  }

  function renderServers() {
    const list = $("mcp-list");
    list.replaceChildren();
    const query = $("mcp-search").value.trim().toLowerCase();
    const matching = servers.filter(server => server.name.toLowerCase().includes(query));
    $("mcp-count").textContent = query ? `${matching.length} / ${servers.length} 个服务` : `${servers.length} 个服务`;
    for (const server of matching) {
      const row = element("article", "mcp-row");
      const label = element("div", "mcp-meta");
      const identity = element("div", "mcp-identity");
      const status = element("span", "status mcp-status", server.transport === "other" ? "不支持验证" : "未验证");
      status.setAttribute("role", "status");
      identity.append(element("strong", "", server.name), status);
      const state = server.enabled ? "已写入本机" : "配置已停用";
      const feedback = element("p", "feedback");
      feedback.setAttribute("role", "status");
      feedback.hidden = true;
      const check = checks.get(server.name);
      if (check) showServerCheck(status, feedback, check);
      const granted = server.permissions?.always_allow.length || 0;
      const access = granted ? ` · 始终允许 ${granted} 项` : "";
      label.append(identity, element("p", "muted", `${transportLabels[server.transport] || transportLabels.other} · ${state}${access}`), feedback);
      const actions = element("div", "button-row");
      const isFrameLeap = selected === "desktop" && server.name === "frameleap";
      if (isFrameLeap && !granted) {
        actions.append(button("更新 Key", openFrameLeap, "secondary"));
      }
      if (server.transport !== "other") {
        if (!granted) {
          const edit = button("编辑", () => openEditor(server), "secondary");
          edit.setAttribute("aria-label", `编辑 ${server.name}`);
          actions.append(edit);
        }
        const verify = button("验证连接", () => verifySaved(server, status, feedback), "secondary");
        verify.setAttribute("aria-label", `验证连接 ${server.name}`);
        actions.append(verify);
      }
      if (server.permissions) {
        const permissions = button("工具权限", () => openPermissions(server), "secondary");
        permissions.setAttribute("aria-label", `工具权限 ${server.name}`);
        actions.append(permissions);
      }
      if (lite.client(selected)?.mcp_toggle) {
        actions.append(button(server.enabled ? "停用" : "启用", () => changeServer("set_mcp_enabled", server.name, !server.enabled), "secondary"));
      }
      const remove = button("移除", () => removeServer(server.name), "text-button danger");
      remove.setAttribute("aria-label", `移除 ${server.name}`);
      actions.append(remove);
      row.append(label, actions);
      list.append(row);
    }
    if (!matching.length) list.append(element("p", "empty", query ? "没有匹配的 MCP 服务。" : "这个范围还没有 MCP 服务。可从热门目录选择，或添加自己的 MCP。"));
    controls();
  }

  function renderCatalog() {
    const list = $("mcp-catalog-list");
    list.replaceChildren();
    const query = $("mcp-catalog-search").value.trim().toLowerCase();
    frameLeap.hidden = !frameLeap.querySelector(".integration-title").textContent.toLowerCase().includes(query);
    if (frameLeap.hidden) frameLeap.open = false;
    if (catalogError) {
      list.append(element("p", "empty", catalogError));
      return;
    }
    for (const card of catalog.filter(card => [card.name, card.id, card.category, card.description].some(value => value.toLowerCase().includes(query)))) {
      const item = element("article", "mcp-card");
      item.dataset.mcp = card.id;
      const top = element("div", "mcp-card-top");
      top.append(element("h3", "", card.name), element("span", "mcp-card-tag", card.category));
      const mode = card.transport === "stdio" ? "本地启动" : transportLabels[card.transport];
      const auth = card.oauth ? "首次使用需登录" : card.input?.directory ? "需选择目录" :
        card.input ? (card.input.required ? "需凭据" : "Key 可选") : "无需填写";
      const description = element("p", "mcp-card-description", card.description);
      const actions = element("div", "mcp-card-actions");
      const existing = servers.find(server => server.name === card.id);
      const unsupported = scope === "project" && card.input?.required && !card.input.directory && !card.input.project_variable;
      const configure = button(existing ? "已配置" : "快速配置", () => {
        if (existing) {
          showView("configured");
          if (existing.permissions?.always_allow.length) openPermissions(existing);
          else if (existing.transport !== "other") void openEditor(existing);
        } else {
          openPreset(card);
        }
      }, existing ? "secondary" : "mcp-card-configure");
      configure.dataset.configure = "";
      if (!existing && unsupported) {
        configure.dataset.unavailable = "";
        configure.textContent = "请切换到用户级";
      }
      configure.setAttribute("aria-label", `${existing ? "查看" : "快速配置"} ${card.name}`);
      const help = button("来源 ↗", async () => {
        try { await invoke("open_mcp_source", { id: card.id }); }
        catch (error) { message(pageFeedback, "error", errorText(error)); }
      }, "text-button");
      help.disabled = !invoke;
      help.setAttribute("aria-label", `${card.name} 官方说明`);
      actions.append(configure, help);
      item.append(top, description, element("p", "muted", `${mode} · ${auth}`), actions);
      list.append(item);
    }
    if (!list.children.length && frameLeap.hidden) list.append(element("p", "empty", "没有匹配的热门 MCP。"));
    controls();
  }

  async function refreshCatalog() {
    if (!invoke || !selected) return;
    const request = ++catalogSequence;
    catalogError = "正在加载热门 MCP…";
    renderCatalog();
    try {
      const result = await invoke("list_mcp_catalog", { client: selected });
      if (request !== catalogSequence) return;
      catalog = result;
      catalogError = "";
    } catch (error) {
      if (request !== catalogSequence) return;
      catalogError = errorText(error);
    }
    renderCatalog();
  }

  async function refreshServers() {
    const request = ++sequence;
    const list = $("mcp-list");
    const client = selected;
    location = null;
    servers = [];
    renderLocation();
    renderCatalog();
    message(targetFeedback, "");
    list.removeAttribute("aria-busy");
    $("mcp-count").textContent = "";
    if (!invoke || !client || scope !== "user" && !project) {
      list.replaceChildren(element("p", "empty", "选择目标客户端及配置范围后，这里会列出已有服务。"));
      controls();
      return;
    }
    list.setAttribute("aria-busy", "true");
    list.replaceChildren(element("p", "empty", "正在读取 MCP 配置…"));
    controls();
    try {
      const result = await invoke("list_mcp", { client, selection: selection() });
      if (request === sequence) {
        location = result;
        servers = result.servers;
        const key = `${client}/${result.location}`;
        const previous = checkLocation === key ? checks : null;
        checkLocation = key;
        checks = new Map();
        for (const server of servers) {
          const check = previous?.get(server.name);
          if (check && check.revision === server.revision && check.enabled === server.enabled) {
            checks.set(server.name, check);
          }
        }
        renderLocation();
        renderServers();
        renderCatalog();
      }
    } catch (error) {
      if (request === sequence) {
        list.replaceChildren();
        message(targetFeedback, "error", errorText(error));
      }
    } finally {
      if (request === sequence) {
        list.removeAttribute("aria-busy");
        controls();
      }
    }
  }

  function refresh() {
    if (busy) return;
    syncTargets();
    if (lite.client("desktop")) void window.refreshFrameLeap?.();
    void refreshCatalog();
    void refreshServers();
  }

  function syncTransport() {
    const local = $("mcp-transport").value === "stdio";
    $("mcp-endpoint-label").textContent = local ? "启动命令或可执行文件路径" : "MCP 服务地址";
    $("mcp-endpoint").placeholder = local ? "例如：node 或 /完整路径/server" : "https://example.com/mcp";
    $("mcp-args-field").hidden = !local;
    $("mcp-options-title").textContent = local ? "环境变量（可选）" : "请求头（可选）";
    $("add-mcp-option").textContent = local ? "＋ 添加环境变量" : "＋ 添加请求头";
    for (const { field, row } of nativeFields) row.hidden = !nativeApplies(field);
    $("mcp-native").hidden = !nativeFields.some(({ field }) => nativeApplies(field));
  }

  function clearEditorFeedback() { message(editorFeedback, ""); }

  function nativeApplies(field) {
    const transport = $("mcp-transport").value;
    return !field.transport || (field.transport === "http" ? transport !== "stdio" : field.transport === transport);
  }

  function initNative(client, saved = {}) {
    nativeFields = (client.mcp_fields || []).map(field => {
      const row = element("label", "field", field.label);
      const input = element(field.kind === "list" ? "textarea" : field.kind === "switch" ? "select" : "input");
      input.spellcheck = false;
      const original = saved[field.key];
      if (field.kind === "switch") {
        input.add(option("使用客户端默认", ""));
        input.add(option("开启", "true"));
        input.add(option("关闭", "false"));
        input.value = original == null ? "" : String(original);
      } else if (field.kind === "list") {
        input.rows = 2;
        input.placeholder = "每行一个工具名";
        input.value = (original || []).join("\n");
      } else if (field.kind === "timeout") {
        input.type = "number";
        input.min = field.key.endsWith("_sec") ? "0" : "1";
        input.max = "2147483647";
        input.step = field.key.endsWith("_sec") ? "any" : "1";
        input.placeholder = "使用客户端默认";
        input.value = original ?? "";
      } else {
        input.maxLength = field.kind === "variable" ? 128 : 4096;
        input.placeholder = field.kind === "variable" ? "例如：MCP_TOKEN" : "可留空";
        input.value = original ?? "";
      }
      row.append(input);
      return { field, row, input, initial: input.value, original };
    });
    $("mcp-native-title").textContent = `${client.name} 高级设置`;
    $("mcp-native-fields").replaceChildren(...nativeFields.map(({ row }) => row));
    $("mcp-native").open = Object.keys(saved).length > 0;
  }

  function nativeInput() {
    const result = Object.create(null);
    for (const { field, input, initial, original } of nativeFields) {
      if (!nativeApplies(field) || (editing && input.value === initial)) continue;
      const raw = input.value.trim();
      if (!raw) {
        if (!editing && input.value === initial && Array.isArray(original) && !original.length) result[field.key] = original;
        else if (editing) result[field.key] = null;
      } else if (field.kind === "timeout") {
        const value = Number(raw);
        if (!Number.isFinite(value) || value <= 0 || value > 2147483647 || !field.key.endsWith("_sec") && !Number.isInteger(value)) {
          throw `请为「${field.label}」输入有效的超时。`;
        }
        result[field.key] = value;
      } else if (field.kind === "switch") {
        result[field.key] = raw === "true";
      } else if (field.kind === "list") {
        result[field.key] = raw.split(/\r?\n/).map(line => line.trim()).filter(Boolean);
      } else {
        result[field.key] = raw;
      }
    }
    return result;
  }

  function loadValue(field, value) {
    field.value = value;
    if (field.value !== value) originalValues.set(field, { raw: value, displayed: field.value });
  }

  function fieldValue(field) {
    const original = originalValues.get(field);
    return original && field.value === original.displayed ? original.raw : field.value;
  }

  function removeField(row) {
    const remove = button("移除", () => {
      const next = row.nextElementSibling?.querySelector("input, textarea") || row.parentElement.nextElementSibling;
      row.remove();
      next.focus();
      clearEditorFeedback();
    }, "text-button danger");
    remove.setAttribute("aria-label", "移除此项");
    return remove;
  }

  function addArgument(value = "") {
    const row = element("div", "mcp-value-row");
    const field = element("textarea");
    field.rows = 1;
    field.spellcheck = false;
    field.setAttribute("aria-label", "启动参数");
    loadValue(field, value);
    row.append(field, removeField(row));
    $("mcp-args").append(row);
    return field;
  }

  function addOption(name = "", value = "") {
    const local = $("mcp-transport").value === "stdio";
    const row = element("div", "mcp-value-row mcp-option-row");
    const key = element("input", "mcp-option-name");
    key.spellcheck = false;
    key.placeholder = local ? "名称" : "请求头名称";
    key.setAttribute("aria-label", local ? "环境变量名称" : "请求头名称");
    loadValue(key, name);
    const field = element("input");
    field.spellcheck = false;
    field.placeholder = "值";
    field.setAttribute("aria-label", "值");
    const useMultiline = local && /[\r\n]/.test(value);
    let editor = field;
    if (useMultiline) {
      editor = element("textarea", "mcp-multiline");
      editor.rows = 2;
      editor.spellcheck = false;
      editor.setAttribute("aria-label", "值");
    }
    field.type = "password";
    loadValue(editor, value);
    editor.hidden = useMultiline;
    const reveal = button("显示", () => {
      if (useMultiline) {
        editor.hidden = !editor.hidden;
        reveal.textContent = editor.hidden ? "显示" : "隐藏";
        if (!editor.hidden) editor.focus();
        return;
      }
      const visible = field.type === "password";
      field.type = visible ? "text" : "password";
      reveal.textContent = visible ? "隐藏" : "显示";
    }, "text-button");
    const placeholder = useMultiline ? element("span", "muted mcp-multiline-placeholder", "多行值") : field;
    row.append(key, placeholder, reveal, removeField(row));
    if (useMultiline) row.append(editor);
    $("mcp-options").append(row);
    return key;
  }

  function populateConnection(client, saved) {
    if (!client.transports.includes(saved.transport)) {
      $("mcp-transport").add(option(transportLabels[saved.transport], saved.transport));
    }
    $("mcp-transport").value = saved.transport;
    $("mcp-endpoint").value = saved.endpoint;
    initNative(client, saved.advanced || {});
    syncTransport();
    for (const arg of saved.args) addArgument(arg);
    for (const [name, value] of Object.entries(saved.options)) addOption(name, value);
  }

  async function openEditor(server = null, draft = null) {
    const client = lite.client(selected);
    if (!canConfigure() || !client) return;
    closePanels();
    editorDestination = destination();
    editing = server?.name || null;
    $("mcp-form-title").textContent = editing ? `编辑 ${editing}` : "添加 MCP 服务";
    $("mcp-form-target").textContent = `保存到 ${targetLabel()}`;
    $("mcp-name").value = editing || draft?.name || "";
    $("mcp-transport").replaceChildren(...client.transports.map(id => option(transportLabels[id], id)));
    if (client.transports.includes(server?.transport)) $("mcp-transport").value = server.transport;
    $("mcp-advanced").open = false;
    if (draft) {
      populateConnection(client, draft);
      $("mcp-advanced").open = Object.keys(draft.options).length > 0;
      if (draft.skipped?.length) {
        message($("mcp-import-result"), "warning", `以下字段没有导入：${draft.skipped.join("、")}。请在目标客户端中核对。`);
      }
      if (draft.name && client.id !== "workbuddy" && !/^[a-zA-Z0-9_-]+$/.test(draft.name)) {
        const result = $("mcp-import-result");
        message(result, "warning", [result.textContent, "请将服务名称改为英文、数字、短横线或下划线。"].filter(Boolean).join(" "));
      }
    } else {
      initNative(client);
      syncTransport();
    }
    form.hidden = false;
    if (editing) {
      busy = true;
      controls();
      message(editorFeedback, "pending", "正在读取连接参数…");
      try {
        const saved = await invoke("get_mcp", { client: client.id, destination: editorDestination, name: editing });
        populateConnection(client, saved);
        revision = saved.revision;
        clearEditorFeedback();
      } catch (error) {
        message(editorFeedback, "error", errorText(error));
      } finally {
        busy = false;
      }
    }
    controls();
    $(editing ? (revision ? "mcp-endpoint" : "close-mcp") : "mcp-name").focus();
  }

  function closeEditor() {
    if (busy) return;
    form.reset();
    $("mcp-args").replaceChildren();
    $("mcp-options").replaceChildren();
    $("mcp-native-fields").replaceChildren();
    nativeFields = [];
    form.hidden = true;
    delete form.dataset.presetOauth;
    editing = null;
    revision = null;
    editorDestination = null;
    message($("mcp-import-result"), "");
    clearEditorFeedback();
  }

  function closeQuick() {
    quickForm.reset();
    quickForm.hidden = true;
    preset = null;
    message(quickFeedback, "");
  }

  function closeImport() {
    importForm.reset();
    importForm.hidden = true;
    message(importFeedback, "");
  }

  function closePanels() {
    closeEditor();
    closeQuick();
    closeImport();
    frameLeap.open = false;
  }

  function loginHint(card, client, name = card.id) {
    if (!card.oauth) return "";
    if (card.transport === "stdio") return "首次启动时，按提示在浏览器中登录。";
    const command = {
      code: "/mcp",
      codex: `codex mcp login ${name}`,
      opencode: `opencode mcp auth ${name}`,
      qwen: "/mcp",
      kimi: `/mcp-config login ${name}`,
      copilot: "/mcp",
    }[client];
    return command ? `首次使用时，运行 ${command} 完成登录。` : "首次使用时，按客户端提示完成登录。";
  }

  function openPreset(card) {
    if (!canConfigure()) return;
    closePanels();
    preset = card;
    $("mcp-quick-title").textContent = `配置 ${card.name}`;
    $("mcp-quick-target").textContent = targetLabel();
    const variable = scope === "project" && card.input && !card.input.directory;
    const hideField = variable && !card.input.project_variable;
    $("mcp-quick-note").textContent = [
      card.requires && `需要 ${card.requires}。`,
      scope === "project" && card.transport === "stdio" && "请确认目标客户端能找到启动命令。",
      loginHint(card, selected),
      variable && (hideField ? "可先不填 Key。需要凭据时请切换到用户级配置。" : "在启动目标客户端的环境里设置该变量，项目文件只写变量名。"),
      card.note,
    ].filter(Boolean).join(" ");
    $("mcp-quick-target-value").textContent = [card.endpoint, ...(card.args || [])].join(" ");
    const input = $("mcp-quick-value");
    $("mcp-quick-field").hidden = !card.input || !!hideField;
    $("mcp-quick-label").textContent = card.input ? card.input.label + (variable ? " 的环境变量名" : "") + (card.input.required ? "" : "（可选）") : "";
    $("pick-mcp-input").hidden = !card.input?.directory;
    input.type = card.input?.directory || variable ? "text" : "password";
    input.placeholder = variable ? "例如：MCP_TOKEN" : "";
    input.readOnly = !!card.input?.directory;
    input.required = !!card.input?.required;
    input.value = "";
    quickForm.hidden = false;
    (card.input?.directory ? $("pick-mcp-input") : card.input && !hideField ? input : $("mcp-quick-title")).focus();
  }

  function formInput() {
    if (!form.reportValidity()) return null;
    const transport = $("mcp-transport").value;
    const options = Object.create(null);
    const names = new Set();
    for (const row of $("mcp-options").children) {
      const key = row.querySelector(".mcp-option-name");
      const field = row.querySelector("[aria-label='值']");
      const name = transport === "stdio" ? fieldValue(key) : key.value.trim();
      if (!name.trim()) throw "请填写环境变量或请求头名称。";
      const comparable = transport === "stdio" ? name : name.toLowerCase();
      if (names.has(comparable)) throw "环境变量或请求头中有重复名称。";
      names.add(comparable);
      options[name] = fieldValue(field);
    }
    const args = transport === "stdio" ? Array.from($("mcp-args").querySelectorAll("textarea"), fieldValue) : [];
    return { name: editing || $("mcp-name").value.trim(), transport, endpoint: $("mcp-endpoint").value.trim(), args, options, revision, advanced: nativeInput() };
  }

  function verifySaved(server, status, feedback) {
    const name = server.name;
    const check = { enabled: server.enabled };
    void verifyServer({ source: "saved", name }, (kind, detail, revision) => {
      if (kind === "pending") checks.set(name, check);
      if (checks.get(name) !== check) return;
      Object.assign(check, { kind, detail, revision: revision ?? server.revision });
      if (status.isConnected) showServerCheck(status, feedback, check);
      else renderServers();
    });
  }

  function openPermissions(server) {
    if (!canConfigure() || !server.permissions || selected !== "desktop") return;
    const existing = server.permissions.always_allow;
    permission = { server, where: destination(), names: new Set(existing), chosen: new Set(existing), busy: false };
    $("mcp-permissions-title").textContent = `工具权限 · ${server.name}`;
    $("mcp-permissions-hint").hidden = false;
    $("mcp-permissions-hint").textContent = existing.length
      ? "如需编辑服务连接，请先清空授权并保存。" : "读取工具列表后，可以逐项选择。";
    message(permissionFeedback, "");
    renderPermissionChoices();
    permissionDialog.showModal();
  }

  function permissionControls() {
    if (!permission) return;
    const { chosen, server, busy } = permission;
    const original = server.permissions.always_allow;
    const changed = original.length !== chosen.size || original.some(name => !chosen.has(name));
    $("mcp-permissions-count").textContent = `已选 ${chosen.size} 项`;
    $("mcp-read-tools").disabled = busy;
    $("mcp-clear-tools").disabled = busy || !chosen.size;
    $("close-mcp-permissions").disabled = busy;
    $("save-mcp-permissions").disabled = busy || !changed;
    $("mcp-permissions-choices").disabled = busy;
  }

  function renderPermissionChoices() {
    const data = permission;
    const choices = Array.from(data.names).sort().map(name => {
      const choice = element("label", "mcp-tool-choice");
      const input = element("input");
      input.type = "checkbox";
      input.checked = data.chosen.has(name);
      input.addEventListener("change", () => {
        if (input.checked) data.chosen.add(name);
        else data.chosen.delete(name);
        permissionControls();
      });
      choice.append(input, element("span", "", name));
      return choice;
    });
    $("mcp-permissions-list").replaceChildren(...choices);
    $("mcp-permissions-choices").hidden = !choices.length;
    permissionControls();
  }

  $("mcp-read-tools").addEventListener("click", async () => {
    const data = permission;
    if (!data || data.busy || !canConfigure()) return;
    data.busy = true;
    permissionControls();
    message(permissionFeedback, "pending", "正在读取工具列表…");
    try {
      const { revision, result } = await invoke("verify_mcp", {
        client: "desktop", destination: data.where, target: { source: "permission", name: data.server.name },
      });
      if (revision !== data.server.revision) throw "服务配置已改变，请关闭窗口并刷新后重试。";
      if ("Err" in result) throw result.Err;
      const names = result.Ok.tool_names || [];
      const original = data.server.permissions.always_allow;
      data.names = new Set([...original, ...names]);
      data.chosen = new Set([...data.chosen].filter(name => data.names.has(name)));
      const hint = result.Ok.tool_count > names.length ? " 部分工具名称无法用于权限匹配。" : "";
      message(permissionFeedback, "success", `已读取 ${names.length} 个可配置的工具。${hint}`);
      $("mcp-permissions-hint").hidden = !original.length;
    } catch (error) {
      message(permissionFeedback, "error", errorText(error));
    } finally {
      data.busy = false;
      renderPermissionChoices();
    }
  });

  $("mcp-clear-tools").addEventListener("click", () => {
    if (!permission || permission.busy) return;
    permission.chosen.clear();
    renderPermissionChoices();
  });
  $("close-mcp-permissions").addEventListener("click", () => permissionDialog.close());
  permissionDialog.addEventListener("cancel", event => { if (permission?.busy) event.preventDefault(); });
  permissionDialog.addEventListener("close", () => { if (!permissionDialog.open) permission = null; });
  $("mcp-permissions").addEventListener("submit", async event => {
    event.preventDefault();
    const data = permission;
    if (!data || $("save-mcp-permissions").disabled || !canConfigure()) return;
    data.busy = true;
    busy = true;
    permissionControls();
    controls();
    try {
      await invoke("save_mcp_permissions", {
        destination: data.where, name: data.server.name, revision: data.server.revision,
        alwaysAllow: [...data.chosen].sort(),
      });
      permissionDialog.close();
      const text = data.chosen.size ? `已保存：${data.server.name} 的 ${data.chosen.size} 项工具始终允许。` : `已清空 Lite 为 ${data.server.name} 设置的授权。`;
      message(pageFeedback, "success", `${text} 请在当前任务结束后完全退出并重新打开 Claude Desktop。`);
      await refreshServers();
      void window.refreshFrameLeap?.();
    } catch (error) {
      message(permissionFeedback, "error", errorText(error));
    } finally {
      data.busy = false;
      busy = false;
      permissionControls();
      controls();
    }
  });

  async function verifyServer(target, show) {
    if (!canConfigure()) return;
    const client = selected;
    const where = target.source === "saved" ? destination() : editorDestination;
    busy = true;
    controls();
    show("pending");
    try {
      const { revision, result } = await invoke("verify_mcp", { client, destination: where, target });
      if ("Err" in result) {
        show("error", result.Err, revision);
        return;
      }
      const count = result.Ok.tool_count;
      const tools = count == null ? "服务未声明工具能力" : `发现 ${count} 个工具`;
      show("success", tools, revision);
    } catch (error) {
      show("error", errorText(error));
    } finally {
      busy = false;
      controls();
    }
  }

  async function removeServer(name) {
    if (!canConfigure()) return;
    if (!window.confirm(`从 ${targetLabel()} 移除「${name}」？`)) return;
    await changeServer("remove_mcp", name);
  }

  async function changeServer(command, name, enabled) {
    if (!canConfigure()) return;
    const client = selected;
    const removed = command === "remove_mcp";
    const where = destination();
    busy = true;
    controls();
    try {
      await invoke(command, { client, destination: where, name, enabled });
      const action = removed ? "移除" : enabled ? "启用" : "停用";
      const feedback = command === "undo_mcp" ? "已撤销上次 MCP 变更。" : `已${action} ${name}。`;
      message(pageFeedback, "success", feedback);
      if (client === "desktop") {
        window.clearFrameLeapFeedback?.();
        void window.refreshFrameLeap?.();
      }
      await refreshServers();
    } catch (error) {
      message(pageFeedback, "error", errorText(error));
    } finally {
      busy = false;
      controls();
      if (removed && editing === name && client === selected) {
        closeEditor();
        controls();
      }
    }
  }

  async function saved(client, name, hint = "") {
    busy = false;
    closePanels();
    showView("configured");
    $("mcp-search").value = "";
    message(pageFeedback, "success", [`已保存 ${name} 到 ${lite.client(client).name}。`, hint].filter(Boolean).join(" "));
    if (client === "desktop") void window.refreshFrameLeap?.();
    await refreshServers();
  }

  form.addEventListener("submit", async event => {
    event.preventDefault();
    if (!canConfigure()) return;
    const client = selected;
    let input;
    try {
      input = formInput();
    } catch (error) {
      message(editorFeedback, "error", errorText(error));
      return;
    }
    if (!input) return;
    busy = true;
    controls();
    message(editorFeedback, "pending", "正在写入客户端配置…");
    try {
      await invoke("save_mcp", { client, destination: editorDestination, input });
      const hint = form.dataset.presetOauth === "true"
        ? loginHint({ id: input.name, transport: input.transport, oauth: true }, client)
        : "";
      await saved(client, input.name, hint);
    } catch (error) {
      message(editorFeedback, "error", errorText(error));
    } finally {
      busy = false;
      controls();
    }
  });

  async function submitPreset(customize) {
    if (!canConfigure() || !preset) return;
    const value = $("mcp-quick-value").value;
    if (preset.input?.required && !value.trim()) {
      message(quickFeedback, "error", `请填写「${preset.input.label}」。`);
      $(preset.input.directory ? "pick-mcp-input" : "mcp-quick-value").focus();
      return;
    }
    const client = selected;
    const card = preset;
    const where = destination();
    busy = true;
    controls();
    message(quickFeedback, "pending", customize ? "正在读取连接参数…" : "正在保存配置…");
    try {
      const input = await invoke("prepare_mcp_preset", { client, scope, id: card.id, value: value || null });
      if (customize) {
        busy = false;
        void openEditor(null, input);
        form.dataset.presetOauth = String(card.oauth);
      } else {
        await invoke("save_mcp", { client, destination: where, input });
        await saved(client, input.name, loginHint(card, client));
      }
    } catch (error) {
      message(quickFeedback, "error", errorText(error));
    } finally {
      busy = false;
      controls();
    }
  }

  quickForm.addEventListener("submit", event => {
    event.preventDefault();
    void submitPreset(false);
  });
  $("edit-mcp-quick").addEventListener("click", () => void submitPreset(true));
  $("close-mcp-quick").addEventListener("click", closeQuick);
  $("mcp-quick-value").addEventListener("input", () => message(quickFeedback, ""));

  async function chooseDirectory(feedback) {
    if (!invoke || busy) return null;
    busy = true;
    controls();
    try {
      return await invoke("pick_mcp_directory");
    } catch (error) {
      message(feedback, "error", errorText(error));
      return null;
    } finally {
      busy = false;
      controls();
    }
  }

  $("pick-mcp-input").addEventListener("click", async () => {
    const picked = await chooseDirectory(quickFeedback);
    if (picked) {
      $("mcp-quick-value").value = picked;
      message(quickFeedback, "");
    }
  });
  $("pick-mcp-project").addEventListener("click", async () => {
    const picked = await chooseDirectory(targetFeedback);
    if (picked && picked !== project) {
      closePanels();
      project = picked;
      resetTarget();
      refresh();
    }
  });

  $("import-mcp").addEventListener("click", () => {
    if (!canConfigure()) return;
    closePanels();
    importForm.hidden = false;
    $("mcp-import-text").focus();
  });
  $("close-mcp-import").addEventListener("click", closeImport);
  $("mcp-import-text").addEventListener("input", () => message(importFeedback, ""));
  importForm.addEventListener("submit", async event => {
    event.preventDefault();
    if (!canConfigure()) return;
    busy = true;
    controls();
    try {
      const input = await invoke("parse_mcp_import", { client: selected, text: $("mcp-import-text").value });
      busy = false;
      void openEditor(null, input);
    } catch (error) {
      message(importFeedback, "error", errorText(error));
    } finally {
      busy = false;
      controls();
    }
  });

  function resetTarget() {
    $("mcp-search").value = "";
    message(pageFeedback, "");
    $("mcp-location").open = false;
  }
  target.addEventListener("change", () => {
    closePanels();
    selected = target.value;
    scope = "user";
    resetTarget();
    refresh();
  });
  $("mcp-scope").addEventListener("change", () => {
    closePanels();
    scope = $("mcp-scope").value;
    resetTarget();
    refresh();
  });
  $("new-mcp").addEventListener("click", () => openEditor());
  $("close-mcp").addEventListener("click", closeEditor);
  $("verify-mcp").addEventListener("click", () => {
    try {
      const input = formInput();
      if (!input) return;
      void verifyServer({ source: "draft", input }, (kind, detail) => {
        const text = {
          pending: "正在验证连接…",
          success: `${checkLabels.success} · ${detail}`,
          error: `${checkLabels.error}。${detail}`,
        }[kind];
        message(editorFeedback, kind, text);
      });
    } catch (error) {
      message(editorFeedback, "error", errorText(error));
    }
  });
  $("add-mcp-arg").addEventListener("click", () => {
    clearEditorFeedback();
    addArgument().focus();
  });
  $("add-mcp-option").addEventListener("click", () => {
    clearEditorFeedback();
    addOption().focus();
  });
  form.addEventListener("input", clearEditorFeedback);
  $("refresh-mcp").addEventListener("click", () => {
    closePanels();
    refresh();
  });
  $("undo-mcp").addEventListener("click", () => void changeServer("undo_mcp"));
  $("mcp-client-settings").addEventListener("click", () => lite.openClientPanel(selected));
  $("mcp-search").addEventListener("input", renderServers);
  $("mcp-catalog-search").addEventListener("input", renderCatalog);
  frameLeap.querySelector("summary").addEventListener("click", event => {
    if (frameLeap.open) return;
    event.preventDefault();
    openFrameLeap();
  });
  $("mcp-show-catalog").addEventListener("click", () => showView("catalog"));
  $("mcp-show-configured").addEventListener("click", () => showView("configured"));
  $("mcp-transport").addEventListener("change", () => {
    clearEditorFeedback();
    const local = $("mcp-transport").value === "stdio";
    if (local === $("mcp-args-field").hidden) {
      $("mcp-endpoint").value = "";
      $("mcp-args").replaceChildren();
      $("mcp-options").replaceChildren();
      $("mcp-advanced").open = false;
    }
    syncTransport();
  });
  window.addEventListener("lite:mcp-target", event => {
    if (!busy) {
      closePanels();
      selected = event.detail;
      scope = "user";
      resetTarget();
      showView("catalog");
    }
    lite.show("tools");
  });
  window.addEventListener("lite:page", event => { if (event.detail === "tools") refresh(); });
  window.addEventListener("lite:refresh", () => {
    if (lite.page === "tools" && !busy && !permissionDialog.open && form.hidden && quickForm.hidden && importForm.hidden) refresh();
  });
  window.addEventListener("lite:frameleap-updated", () => {
    const invalidated = checkLocation.startsWith("desktop/") && checks.delete("frameleap");
    if (selected !== "desktop") return;
    if (!busy) void refreshServers();
    else if (invalidated) renderServers();
  });
  controls();
})();

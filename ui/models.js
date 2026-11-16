"use strict";

(() => {
  const { $, invoke, element, button, option, message, errorText } = lite;
  const form = $("model-form");
  const desktopList = $("desktop-model-list");
  const modelIdInput = $("model-id");
  const manualIdsInput = $("manual-model-ids");
  const library = $("model-library");
  const modelList = $("model-list");
  const providerIndex = $("library-providers");
  const providerPicker = $("library-provider");
  const modelTabs = document.querySelectorAll("[data-model-tab]");
  const emptyFilters = () => ({ query: "", target: "", provider: "" });
  const filters = { catalog: emptyFilters(), saved: emptyFilters() };
  const expandedProviders = new Set();
  const removingModels = new Set();
  const catalogPageSize = 150;
  const codeInputs = Object.fromEntries(Object.entries(lite.codeRoles).map(([role, name]) => {
    const field = element("label", "field", name);
    const input = element("input");
    input.maxLength = 256;
    input.spellcheck = false;
    input.placeholder = "沿用默认模型";
    input.setAttribute("list", "code-model-catalog");
    field.append(input);
    $("code-model-fields").append(field);
    return [role, input];
  }));
  let tab = "catalog";
  let editor = null;
  let busy = false;
  let candidates = [];
  let candidateLimit = catalogPageSize;
  let source = "preset";
  let partial = false;

  function accountFor(model) {
    return lite.data.accounts.find(account => account.id === model.account);
  }

  function providerId(entry) { return entry?.provider || entry?.preset || entry?.id || "other"; }
  function providers() {
    return lite.data.presets.filter(preset => preset.id === providerId(preset));
  }
  function apiModelId(id) { return id.trim().replace(/\[1m\]$/, ""); }
  function profileModels(model, target) {
    const ids = [model.model];
    if (!target || target === "desktop") ids.push(...(model.desktop_models || []));
    if (!target || target === "code") ids.push(...Object.values(model.code_models || {}));
    return [...new Set(ids.filter(Boolean).map(apiModelId))]
      .filter(id => id && (target !== "desktop" || !isAuto(id)));
  }

  function clientNames(ids) {
    return ids.map(id => lite.client(id)?.name).filter(Boolean).join(" · ");
  }

  function directoryGroups(entries, target = "", terms = []) {
    const groups = new Map();
    for (const { model, account, preset } of entries) {
      if (target && !model.clients.includes(target)) continue;
      const id = providerId(account);
      const name = preset?.name || account.name;
      const ids = profileModels(model, target).filter(modelId => matchesSearch(terms, name, id, account.name, model.name, modelId));
      if (!ids.length) continue;
      if (!groups.has(id)) groups.set(id, { id, name, preset, models: new Set(), clients: new Set() });
      const group = groups.get(id);
      for (const modelId of ids) group.models.add(modelId);
      for (const client of model.clients) group.clients.add(client);
    }
    return [...groups.values()].sort((left, right) => left.name.localeCompare(right.name, "zh-CN"));
  }

  function syncFilters() {
    const current = filters[tab];
    const clients = lite.data.clients;
    const select = $("library-client");
    select.replaceChildren(option("所有客户端", ""), ...clients.map(client => option(client.name, client.id)));
    if (!clients.some(client => client.id === current.target)) current.target = "";
    select.value = current.target;
    select.disabled = !clients.length;
    const entries = directoryGroups(savedEntries());
    if (!entries.some(item => item.id === current.provider)) current.provider = "";
    providerPicker.replaceChildren(option("全部服务商", ""), ...entries.map(item => option(item.name, item.id)));
    $("library-provider-field").hidden = !entries.length;
    const search = $("model-search");
    if (search.value !== current.query) search.value = current.query;
    search.placeholder = "搜索名称、模型 ID 或服务商";
  }

  function searchText(value) { return value.normalize("NFKC").toLowerCase(); }
  function searchTerms(query) { return searchText(query).trim().split(/\s+/).filter(Boolean); }

  function matchesSearch(terms, ...parts) {
    if (!terms.length) return true;
    const text = searchText(parts.join(" "));
    return terms.every(term => text.includes(term));
  }

  function renderProviderIndex(providers, groups, selected, target) {
    const counts = new Map(groups.map(({ id, models }) => [id, models.size]));
    const total = groups.reduce((sum, { models }) => sum + models.size, 0);
    const entries = [{ id: "", name: "全部服务商" }, ...providers];
    providerIndex.replaceChildren(...entries.map(({ id, name, clients }) => {
      const control = button("", () => changeFilter("provider", id), "provider-filter");
      control.id = `provider-filter-${id || "all"}`;
      control.setAttribute("aria-pressed", String(selected === id));
      const available = !target || !id || clients.has(target);
      control.disabled = !available && selected !== id;
      if (!available) control.title = "此服务商不支持当前客户端";
      control.append(element("span", "provider-filter-name", name));
      const count = element("span", "provider-filter-count", String(id ? (counts.get(id) ?? 0) : total));
      count.setAttribute("aria-hidden", "true");
      control.append(count);
      return control;
    }));
  }

  function showConfigured(provider, id, target) {
    filters.saved = { query: id || "", provider, target };
    setTab("saved");
    $("model-search").focus();
  }

  function providerPanel({ id: provider, name, preset, models, clients }, target) {
    const panel = element("article", "provider-panel");
    const heading = element("header", "provider-heading");
    const title = element("h2", "", name);
    title.id = `provider-title-${provider}`;
    panel.setAttribute("aria-labelledby", title.id);
    const actions = element("div", "provider-actions");
    heading.append(title, actions);
    panel.append(heading);
    if (preset) {
      const load = button("拉取模型", event => openEditor({
        target, preset: preset.id, fetchCatalog: true, opener: event.currentTarget,
      }), "model-configure");
      load.id = `load-provider:${provider}`;
      load.setAttribute("aria-label", `拉取 ${name} 模型`);
      actions.append(load);
    }
    const supported = clientNames([...clients]);
    if (supported) {
      const capabilities = element("p", "provider-capabilities", supported);
      capabilities.id = `provider-clients-${provider}`;
      capabilities.hidden = !expandedProviders.has(provider);
      const disclosure = button("兼容客户端", () => {
        capabilities.hidden = !capabilities.hidden;
        if (capabilities.hidden) expandedProviders.delete(provider);
        else expandedProviders.add(provider);
        disclosure.setAttribute("aria-expanded", String(!capabilities.hidden));
      }, "provider-disclosure");
      disclosure.id = `provider-details-${provider}`;
      disclosure.setAttribute("aria-controls", capabilities.id);
      disclosure.setAttribute("aria-expanded", String(!capabilities.hidden));
      actions.append(disclosure);
      panel.append(capabilities);
    }
    const rows = element("ul", "provider-models");
    rows.setAttribute("role", "list");
    for (const id of models) {
      const row = element("li", "provider-model");
      const configure = button("已配置", () => showConfigured(provider, id, target), "model-configure configured-model");
      configure.id = `configured:${provider}:${id}`;
      configure.setAttribute("aria-label", `查看已配置模型 ${id}`);
      configure.setAttribute("aria-describedby", title.id);
      row.append(element("code", "", id), configure);
      rows.append(row);
    }
    panel.append(rows);
    return panel;
  }

  function savedEntries(terms = [], target = "", provider = "") {
    const entries = [];
    for (const model of lite.data.models) {
      const account = accountFor(model);
      if (!account?.has_key || (target && !model.clients.includes(target)) || (provider && providerId(account) !== provider)) continue;
      const preset = providers().find(preset => preset.id === providerId(account));
      if (matchesSearch(terms, model.name, account.name, preset?.name, account.preset,
        ...profileModels(model, target))) entries.push({ model, account, preset });
    }
    return entries;
  }

  async function removeModel(model, control) {
    if (removingModels.has(model.id)) return;
    const confirmed = await lite.confirmAction(`删除「${model.name}」？`, "将从模型库移除此档案。", "确认删除");
    if (!confirmed) return;
    removingModels.add(model.id);
    control.disabled = true;
    const feedback = $("models-feedback");
    message(feedback, "pending", "正在删除档案…");
    try {
      await invoke("remove_model", { id: model.id });
      lite.data.models = lite.data.models.filter(item => item.id !== model.id);
      syncFilters();
      message(feedback, "success", "模型档案已删除。");
      void lite.refresh();
    } catch (error) {
      message(feedback, "error", errorText(error));
    } finally {
      removingModels.delete(model.id);
      renderLibrary();
    }
  }

  function savedCard({ model, account, preset }, target) {
    const row = element("article", "saved-model");
    const content = element("div", "model-meta");
    const heading = element("h2", "", model.name);
    heading.id = `saved-model:${model.id}`;
    heading.tabIndex = -1;
    const source = Array.from(new Set([preset?.name, account?.name].filter(Boolean))).join(" · ");
    const identity = element("div", "saved-model-identity");
    if (source) identity.append(element("span", "", source));
    if (model.model !== model.name) identity.append(element("code", "", model.model));
    const details = element("div", "saved-model-details");
    const used = lite.data.in_use.includes(model.id);
    if (used) {
      const pending = lite.data.clients.some(client => client.status.pending?.id === model.id);
      details.append(element("span", "status", pending ? "等待应用" : "使用中"));
    }
    if (model.checked) {
      const checked = element("span", "status", model.checked.healthy ? "接口测试通过" : "接口测试未通过");
      checked.dataset.state = model.checked.healthy ? "ready" : "unavailable";
      details.append(checked);
    }
    if (model.desktop_models?.length) details.append(element("span", "", `Desktop · ${model.desktop_models.length} 个可选模型`));
    const clients = clientNames(model.clients);
    content.append(heading, identity, details);
    if (clients) content.append(element("p", "provider-capabilities", clients));
    const extraModels = profileModels(model).filter(id => id !== apiModelId(model.model));
    if (extraModels.length) {
      const list = element("ul", "saved-profile-models");
      list.setAttribute("aria-label", "此档案中的其他模型");
      for (const id of extraModels) {
        const item = element("li");
        item.append(element("code", "", id));
        list.append(item);
      }
      content.append(list);
    }

    const actions = element("div", "button-row");
    const client = target ? lite.client(target) : model.clients.map(id => lite.client(id)).find(lite.canApply);
    const use = button("前往应用", () => {
      if (client.agent) {
        lite.addAgentDraft(client.id, [model.id]);
        $(`agent-choice-${model.id}`)?.focus();
      } else {
        lite.selectProfile(client.id, model.id);
        lite.showClient(client.id);
        $("client-model-picker").focus();
      }
    }, "model-configure");
    use.disabled = !lite.canApply(client);
    if (client) use.title = `在 ${client.name} 中选用`;
    const edit = button("编辑", event => openEditor({ model, opener: event.currentTarget }), "secondary");
    edit.id = `edit-model:${model.id}`;
    const remove = button("删除", event => removeModel(model, event.currentTarget), "text-button danger");
    remove.disabled = used || removingModels.has(model.id);
    if (used) remove.title = "模型正在使用或等待应用";
    for (const action of [use, edit, remove]) action.setAttribute("aria-describedby", heading.id);
    actions.append(use, edit, remove);
    row.append(content, actions);
    return row;
  }

  function libraryEmpty(title, detail) {
    const state = element("div", "library-empty");
    state.append(element("h2", "", title));
    if (detail) state.append(detail);
    return state;
  }

  function renderLibrary() {
    const active = document.activeElement;
    const focused = modelList.contains(active) || providerIndex.contains(active) ? active.id : "";
    const directory = tab === "catalog";
    const { query, target, provider } = filters[tab];
    const terms = searchTerms(query);
    const ready = lite.data.presets.length > 0;
    const summary = $("library-summary");
    library.dataset.tab = tab;
    const profiles = savedEntries();
    const providerGroups = directoryGroups(profiles);
    const configured = providerGroups.reduce((sum, { models }) => sum + models.size, 0);
    $("saved-count").textContent = String(configured);
    $("saved-count").title = `${configured} 个已配置模型`;
    $("new-model").disabled = !invoke || busy || !ready;
    $("open-model-switch").disabled = !lite.data.clients.length;
    $("library-provider-index").hidden = !directory || !providerGroups.length;
    $("clear-library-filters").hidden = !query && !target && !provider;
    if (providerPicker.value !== provider) providerPicker.value = provider;
    if (directory) {
      for (const option of providerPicker.options) {
        const entry = providerGroups.find(group => group.id === option.value);
        option.disabled = !!target && !!entry && entry.id !== provider && !entry.clients.has(target);
      }
    }
    for (const control of modelTabs) control.setAttribute("aria-pressed", String(control.dataset.modelTab === tab));
    if (!ready) {
      const failed = !$("workspace-error").hidden;
      summary.textContent = failed ? "模型库暂时无法读取" : "正在读取模型库…";
      modelList.replaceChildren();
      if (failed && invoke) modelList.append(button("重新读取", event => {
        event.currentTarget.disabled = true;
        void lite.refresh();
      }, "secondary"));
      return;
    }
    if (directory) {
      const groups = directoryGroups(profiles, target, terms);
      const visible = groups.filter(({ id }) => !provider || provider === id);
      const count = visible.reduce((sum, { models }) => sum + models.size, 0);
      renderProviderIndex(providerGroups, groups, provider, target);
      summary.textContent = `${count} 个已配置模型`;
      modelList.replaceChildren(...visible.map(group => providerPanel(group, target)));
      const selected = target && providerGroups.find(entry => entry.id === provider);
      if (selected && !selected.clients.has(target)) {
        modelList.append(libraryEmpty("未找到匹配的模型", element("p", "muted", `${selected.name} 没有适用于 ${lite.client(target).name} 的已配置模型。`)));
      }
    } else {
      const entries = savedEntries(terms, target, provider);
      const total = profiles.length;
      summary.textContent = entries.length === total ? `${total} 个已配置档案` : `${entries.length} / ${total} 个已配置档案`;
      modelList.replaceChildren(...entries.map(entry => savedCard(entry, target)));
    }
    if (!profiles.length) {
      const add = button("添加模型", event => openNewModel(event.currentTarget), "secondary");
      add.id = "add-first-model";
      add.disabled = !invoke || busy;
      modelList.append(libraryEmpty("还没有配置模型", add));
    }
    if (!modelList.childElementCount) modelList.append(libraryEmpty("未找到匹配的模型"));
    if (focused) $(focused)?.focus({ preventScroll: true });
  }

  function changeFilter(key, value) {
    filters[tab][key] = value;
    message($("models-feedback"), "");
    renderLibrary();
    window.scrollTo(0, 0);
  }

  function setTab(next) {
    tab = next;
    syncFilters();
    renderLibrary();
    window.scrollTo(0, 0);
  }

  function openNewModel(opener) {
    const { target, provider } = filters[tab];
    const preset = providers().find(item => item.id === provider);
    openEditor({ target, preset: preset && (!target || preset.clients.includes(target)) ? preset.id : "", opener });
  }

  function currentAccount() { return lite.data.accounts.find(account => account.id === $("account-picker").value); }
  function currentPreset() {
    const account = currentAccount();
    const id = $("provider-picker").value || providerId(account);
    return lite.data.presets.find(preset => preset.id === id);
  }
  function target() { return lite.client(editor?.target || "-"); }
  function isAuto(id) { return apiModelId(id).toLowerCase() === "auto"; }
  function supportsCandidate(id) { return !editor.desktop || !isAuto(id); }

  function modelLines(text) {
    return text.split(/\r?\n/).map(line => line.trim()).filter(Boolean);
  }

  function mergeModelIds(current, added) {
    const ids = new Map(current.map(id => [apiModelId(id), id]));
    for (const id of added) {
      const key = apiModelId(id);
      if (!ids.has(key)) ids.set(key, id);
    }
    return [...ids.values()];
  }

  function desktopModelsForDraft(selected, primary) {
    if (!editor.desktop) return $("desktop-model-options").hidden ? null : modelLines(desktopList.value);
    // A profile with one implicit Desktop model can keep using its model ID.
    if (!editor.hadDesktopList && selected.length === 1 && apiModelId(selected[0]) === apiModelId(primary)) return [];
    return selected.map(apiModelId);
  }

  function draftModels() {
    const selected = selectedModels();
    const primary = editor.id ? modelIdInput.value.trim() : selected[0] || "";
    const desktopModels = desktopModelsForDraft(selected, primary);
    return { selected, desktopModels, model: primary || desktopModels?.[0] || "" };
  }

  function setCandidates(models, kind, limited = false) {
    const unique = new Map();
    for (const model of models) {
      const entry = typeof model === "string" ? { id: model, name: model } : model;
      if (typeof entry.id !== "string" || !apiModelId(entry.id) || /\s/.test(entry.id)) continue;
      unique.set(apiModelId(entry.id), entry);
    }
    candidates = [...unique.values()];
    $("code-model-catalog").replaceChildren(...candidates.map(model => option(model.name || model.id, model.id)));
    source = kind;
    partial = limited;
    if (kind === "preset") $("candidate-search").value = "";
    refreshCandidateResults();
  }

  function selectCandidate(id) {
    modelIdInput.value = id;
    message($("editor-feedback"), "");
    editorControls();
  }

  function selectedModels() {
    if (!editor.desktop) return editor.selected;
    if (editor.desktopSelection !== null) return editor.desktopSelection;
    const primary = modelIdInput.value.trim();
    return primary && supportsCandidate(primary) ? [primary] : [];
  }

  function syncCandidateChecks() {
    const primary = apiModelId(modelIdInput.value);
    const selected = new Set(selectedModels().map(apiModelId));
    for (const control of $("candidate-list").querySelectorAll("[data-model]")) {
      const id = apiModelId(control.dataset.model);
      if (control.type === "checkbox") {
        control.checked = selected.has(id);
      } else {
        control.setAttribute("aria-pressed", String(primary === id));
        control.lastElementChild.textContent = primary === id ? "当前" : "使用";
      }
    }
  }

  function updateSelection(models) {
    if (editor.desktop) editor.desktopSelection = models;
    else editor.selected = models;
    message($("editor-feedback"), "");
    editorControls();
  }

  function toggleCandidate(id, checked) {
    const selected = selectedModels();
    const key = apiModelId(id);
    updateSelection(checked ? mergeModelIds(selected, [id]) : selected.filter(model => apiModelId(model) !== key));
  }

  function addManualModels() {
    const ids = modelLines(manualIdsInput.value);
    if (!ids.length) return true;
    if (ids.some(id => !apiModelId(id) || /[\s\u0000-\u001f\u007f-\u009f]/u.test(id) || new TextEncoder().encode(id).length > 256)) {
      message($("editor-feedback"), "error", "请每行填写一个有效的模型 ID，模型 ID 内不能有空格。");
    } else if (ids.some(id => !supportsCandidate(id))) {
      message($("editor-feedback"), "error", "Claude Desktop 需要具体的模型 ID。请移除 auto。");
    } else {
      manualIdsInput.value = "";
      updateSelection(mergeModelIds(selectedModels(), ids));
      return true;
    }
    $("manual-models").open = true;
    manualIdsInput.focus();
    return false;
  }

  function renderSelectedModels(selected) {
    $("selected-models").hidden = !!editor.id && !editor.desktop;
    $("selected-title").textContent = editor.desktop ? "Claude Desktop 模型" : "已选模型";
    $("selected-count").textContent = `已选 ${selected.length} 个`;
    const empty = $("selected-empty");
    const canUseProfile = editor.desktop && !editor.target && modelIdInput.value.trim() && supportsCandidate(modelIdInput.value);
    empty.textContent = canUseProfile ? "列表留空时使用档案模型 ID。" : "从目录勾选模型，或手动添加模型 ID。";
    empty.hidden = !!selected.length;
    const list = $("selected-model-list");
    list.replaceChildren();
    if ($("selected-models").hidden) return;
    for (const [index, id] of selected.entries()) {
      const row = element("li");
      const actions = element("div", "button-row");
      if (index === 0) {
        actions.append(element("span", "default-model-tag", "默认"));
      } else {
        const makeDefault = button("设为默认", () => {
          updateSelection([id, ...selected.filter(model => model !== id)]);
          list.scrollTop = 0;
          list.firstElementChild?.querySelector("button")?.focus({ preventScroll: true });
        }, "text-button");
        makeDefault.disabled = busy;
        makeDefault.setAttribute("aria-label", `将 ${id} 设为默认`);
        actions.append(makeDefault);
      }
      const remove = button("移除", () => {
        toggleCandidate(id, false);
        const next = list.children[Math.min(index, list.children.length - 1)];
        (next?.querySelector("button:last-child") || $("candidate-search")).focus();
      }, "text-button");
      remove.disabled = busy;
      remove.setAttribute("aria-label", `移除 ${id}`);
      actions.append(remove);
      row.append(element("code", "", id), actions);
      list.append(row);
    }
  }

  function renderCandidates() {
    const list = $("candidate-list");
    const terms = searchTerms($("candidate-search").value);
    const options = connectionReady() ? candidates.filter(model => supportsCandidate(model.id)) : [];
    const matches = options.filter(model => matchesSearch(terms, model.name, model.id));
    list.replaceChildren();
    const multiple = !editor.id || editor.desktop;
    for (const model of matches.slice(0, candidateLimit)) {
      const row = multiple ? element("label", "candidate-option") : button("", () => selectCandidate(model.id), "candidate-option");
      if (multiple) {
        const check = element("input");
        check.type = "checkbox";
        check.dataset.model = model.id;
        check.addEventListener("change", () => toggleCandidate(model.id, check.checked));
        row.append(check);
      } else {
        row.dataset.model = model.id;
        row.setAttribute("aria-label", `使用 ${model.name || model.id}`);
      }
      const label = element("span");
      label.append(element("strong", "", model.name || model.id));
      if (model.name && model.name !== model.id) label.append(element("small", "", model.id));
      row.append(label);
      if (!multiple) row.append(element("span", "candidate-action"));
      list.append(row);
    }
    if (matches.length > candidateLimit) {
      list.append(button("显示更多模型", () => {
        const next = matches[candidateLimit];
        candidateLimit += catalogPageSize;
        renderCandidates();
        Array.from(list.querySelectorAll("[data-model]"))
          .find(control => control.dataset.model === next.id)?.focus();
      }, "candidate-more"));
    }
    syncCandidateChecks();
    if (!matches.length) list.append(element("p", "empty", terms.length && options.length
      ? "当前列表没有匹配的模型。" : "可手动填写服务商提供的模型 ID。"));
    const summary = source === "remote"
      ? `账号目录 · ${options.length} 个模型${partial ? "（部分目录）" : ""}`
      : options.length ? `预置参考 · ${options.length} 个模型${currentPreset()?.id === "qwen" ? "（以套餐权限为准）" : ""}`
        : "当前没有可搜索的模型";
    $("catalog-source").textContent = terms.length ? `${summary} · 匹配 ${matches.length} 个` : summary;
  }

  function refreshCandidateResults() {
    candidateLimit = catalogPageSize;
    renderCandidates();
    $("candidate-list").scrollTop = 0;
  }

  function connectionReady() {
    const account = currentAccount();
    if (account) return account.has_key || !!$("provider-key").value.trim();
    const preset = currentPreset();
    return !!(preset && $("provider-key").value.trim() && (preset.id !== "custom" || $("provider-url").value.trim()));
  }

  function resetModelSelection() {
    editor.selected = editor.initialModel ? [editor.initialModel] : [];
    editor.desktopSelection = null;
    modelIdInput.value = editor.initialModel;
    manualIdsInput.value = "";
  }

  function saveBeforeApplying() {
    return ["desktop", "codex"].includes(editor?.target);
  }

  function editorControls() {
    if (!editor) return;
    for (const input of form.querySelectorAll("button, input, select, textarea")) input.disabled = busy;
    $("provider-picker").disabled = busy || !!editor.id;
    const { selected, desktopModels, model } = draftModels();
    const current = target();
    const probeId = (editor.desktop && selected[0]) || (editor.target === "desktop" ? "" : model);
    if (!busy) {
      const ready = connectionReady();
      for (const id of ["fetch-models", "skip-catalog", "refresh-catalog"]) $(id).disabled = !ready;
      $("probe-model").disabled = !ready || !probeId;
      $("save-model").disabled = !ready || !model || (editor.target === "desktop" && !selected.length);
      $("add-manual-model").disabled = !manualIdsInput.value.trim();
      const listed = new Set((desktopModels || []).map(apiModelId));
      $("add-desktop-candidate").disabled = !selected.some(id => !isAuto(id) && !listed.has(apiModelId(id)));
      $("provider-auth").disabled = $("provider-protocol").value !== "anthropic";
    }
    $("probe-model").title = probeId ? `测试 ${probeId}` : "";
    form.setAttribute("aria-busy", String(busy));
    const context = new Set([current?.name, currentPreset()?.name, currentAccount()?.name].filter(Boolean));
    $("editor-target").textContent = Array.from(context).join(" · ");
    $("editor-target").hidden = !context.size;
    $("model-id-field").hidden = !editor.id || editor.target === "desktop";
    $("model-id-label").textContent = editor.desktop ? "档案模型 ID"
      : ["code", "codex"].includes(current?.id) ? "默认模型 ID" : "模型 ID";
    $("profile-model-help").hidden = !editor.desktop;
    syncCandidateChecks();
    renderSelectedModels(selected);
    const save = $("save-model");
    if (!editor.id && selected.length > 1) {
      const count = selected.length;
      save.textContent = current?.id === "workbuddy" && lite.canApply(current)
        ? `保存并添加 ${count} 个模型` : current?.id === "code" && lite.canApply(current)
          ? `保存 ${count} 个模型并应用默认` : `保存 ${count} 个模型`;
    } else {
      save.textContent = current?.agent || saveBeforeApplying() || !lite.canApply(current)
        ? "保存档案" : current.id === "workbuddy" ? "保存并添加" : "保存并应用";
    }
    $("new-model").disabled = busy || !lite.data.presets.length;
  }

  function syncConnection({ resetOnProviderChange = false } = {}) {
    const account = currentAccount();
    const preset = currentPreset();
    const presetId = preset?.id || "";
    const custom = !account && preset?.id === "custom";
    const protocols = preset?.protocols || [];
    const protocol = account?.protocol || (custom ? target()?.protocol : null) || protocols[0] || "anthropic";
    $("account-options").hidden = !!account;
    $("connection-details").hidden = !custom;
    const supported = target()?.protocols;
    $("protocol-field").hidden = !!target() && !(custom && supported?.length > 1);
    for (const option of $("provider-protocol").options) {
      option.disabled = !!supported && !supported.includes(option.value);
    }
    $("provider-protocol").value = protocol;
    $("provider-url").value = account?.base_url || preset?.base_url || "";
    $("provider-auth").value = account?.auth || preset?.auth || "auth_token";
    $("auth-field").hidden = protocol !== "anthropic";
    $("provider-key").value = "";
    $("provider-key").placeholder = account?.has_key ? "使用已保存的 Key" : "粘贴从服务商获取的 Key";
    $("provider-key-help").textContent = account?.has_key
      ? "留空继续使用已保存的 Key。填写新 Key 会更新该接入账号。"
      : presetId === "qwen" ? "请从千问 Token Plan 订阅页复制以 sk-sp- 开头的专属 API Key。"
        : account ? "请为此接入账号重新填写 API Key。"
        : preset ? `在 ${preset.name} 的控制台创建或复制 API Key。` : "先选一个服务商，再粘贴它提供的 API Key。";
    if (resetOnProviderChange && editor.preset !== presetId) {
      $("model-name").value = "";
      resetModelSelection();
      desktopList.value = "";
      for (const input of Object.values(codeInputs)) input.value = "";
    }
    editor.preset = presetId;
    updateModelOptions();
    const savedPreset = account && lite.data.presets.find(item => item.id === account.preset);
    const options = savedPreset?.models || preset?.models || [];
    setCandidates(options, "preset");
    editorControls();
  }

  function updateModelOptions() {
    const client = target();
    const preset = currentPreset();
    const anthropic = (currentAccount()?.clients || (preset?.id === "custom" ? [] : preset?.clients) || []).includes("desktop")
      || $("provider-protocol").value === "anthropic" && (!preset || preset.id === "custom");
    editor.desktop = editor.target === "desktop" || (!client && !!editor.id && anthropic);
    $("desktop-model-options").hidden = !anthropic || !!client || editor.desktop;
    $("code-model-options").hidden = !anthropic || (!!client && client.id !== "code");
  }

  function step(next) {
    const select = next === "selection";
    for (const suffix of ["step", "actions"]) {
      $(`connection-${suffix}`).hidden = select;
      $(`selection-${suffix}`).hidden = !select;
    }
    for (const item of form.querySelectorAll("[data-step]")) {
      if (item.dataset.step === next) item.setAttribute("aria-current", "step");
      else item.removeAttribute("aria-current");
    }
    editorControls();
    window.scrollTo(0, 0);
    if (!select) {
      $(currentAccount() || currentPreset() ? "provider-key" : "provider-picker").focus({ preventScroll: true });
    } else if (candidates.some(model => supportsCandidate(model.id))) {
      $("candidate-search").focus({ preventScroll: true });
    } else if (!$("selected-models").hidden) {
      $("manual-models").open = true;
      manualIdsInput.focus();
    } else {
      modelIdInput.focus();
    }
  }

  function syncAccounts(preferredId) {
    const preset = currentPreset();
    const targetId = editor.target;
    const accounts = lite.data.accounts.filter(account =>
      (!preset || providerId(account) === preset.id) && (!targetId || account.clients.includes(targetId)));
    const reusable = accounts.filter(account => account.has_key);
    const existing = preferredId === undefined && preset?.id !== "custom" && reusable.length === 1
      ? reusable[0] : accounts.find(account => account.id === preferredId);
    $("account-picker").replaceChildren(option("使用新的 API Key", ""),
      ...accounts.map(account => option(account.has_key ? account.name : `${account.name}（需填写 Key）`, account.id)));
    $("account-picker").value = existing?.id || "";
    $("account-field").hidden = !accounts.length;
    return existing;
  }

  function openEditor({ model, target: targetId, preset: presetId, accountId, modelId, showOptions = false,
    candidates: catalog, fetchCatalog = false, fromClient = false, opener = document.activeElement } = {}) {
    if (!invoke || busy || !lite.data.presets.length) return;
    const scrollY = window.scrollY;
    lite.show("models");
    const initialModel = model?.model || modelId || "";
    const desktopModels = model?.desktop_models || [];
    editor = {
      id: model?.id || null, target: targetId || "", fromClient, opener: opener?.id, scrollY,
      initialModel, selected: !model && initialModel ? [initialModel] : [],
      desktopSelection: desktopModels.length ? desktopModels : null, hadDesktopList: !!desktopModels.length,
    };
    const current = target();
    const saved = model ? accountFor(model) : lite.data.accounts.find(account => account.id === accountId);
    const presets = providers().filter(preset => !current || preset.clients.includes(current.id));
    const provider = saved ? providerId(saved) : presetId;
    const choices = presets.map(preset => option(preset.name, preset.id));
    if (saved && !presets.some(preset => preset.id === provider)) choices.push(option(saved.name, provider));
    $("provider-picker").replaceChildren(option("选择服务商", ""), ...choices);
    $("provider-picker").value = provider || "";
    const existing = syncAccounts(saved?.id ?? (provider ? undefined : ""));
    editor.account = existing?.id || "";
    modelIdInput.value = initialModel;
    $("model-name").value = model?.name || "";
    desktopList.value = desktopModels.join("\n");
    $("desktop-model-options").open = !!desktopModels.length;
    $("code-model-options").open = targetId === "code" && (showOptions || Object.values(model?.code_models || {}).some(Boolean));
    for (const [role, input] of Object.entries(codeInputs)) input.value = model?.code_models?.[role] || "";
    manualIdsInput.value = "";
    $("manual-models").open = false;
    $("account-name").value = "";
    $("account-options").open = false;
    $("form-title").textContent = model ? "编辑模型档案" : "配置新模型";
    $("close-model").textContent = editor.fromClient ? "返回客户端" : "返回模型库";
    message($("editor-feedback"), "");
    syncConnection();
    if (catalog && existing) setCandidates(catalog.models, "remote", catalog.partial);
    library.hidden = true;
    $("model-header-actions").hidden = true;
    form.hidden = false;
    const skipConnection = existing?.has_key && (model || modelId || showOptions || catalog);
    step(skipConnection ? "selection" : "connection");
    if (fetchCatalog && existing?.has_key) void loadCatalog();
  }

  function closeEditor(returnToClient = false) {
    if (!editor || busy) return;
    const previous = editor;
    editor = null;
    form.reset();
    form.hidden = true;
    library.hidden = false;
    $("model-header-actions").hidden = false;
    if (returnToClient && previous.fromClient) lite.showClient(previous.target);
    renderLibrary();
    if (returnToClient) {
      window.scrollTo(0, previous.scrollY);
      ($(previous.opener) || $(previous.fromClient ? "clients-title" : "model-search")).focus({ preventScroll: true });
    }
  }

  function draft(test = false) {
    const { selected, desktopModels, model } = draftModels();
    const codeModels = $("code-model-options").hidden ? null : Object.fromEntries(
      Object.entries(codeInputs).map(([role, input]) => [role, input.value]),
    );
    return {
      id: editor.id,
      account_id: $("account-picker").value || null,
      preset: $("provider-picker").value,
      account_name: $("account-name").value,
      base_url: $("provider-url").value,
      auth: $("provider-auth").value,
      protocol: $("provider-protocol").value,
      key: $("provider-key").value,
      name: $("model-name").value || (!editor.id && selected.length > 1 ? candidateName(model) : ""),
      model: test && editor.desktop && selected.length ? selected[0] : model,
      desktop_models: desktopModels ?? (editor.account !== $("account-picker").value ? [] : null),
      code_models: codeModels ?? (editor.account !== $("account-picker").value ? {} : null),
    };
  }

  function candidateName(id) {
    const name = candidates.find(model => apiModelId(model.id) === apiModelId(id))?.name;
    return name && name.length <= 96 ? name : "";
  }

  async function loadCatalog() {
    if (busy || !editor || !connectionReady()) return;
    const fromConnection = !$("connection-step").hidden;
    const input = draft();
    const client = editor.target || null;
    busy = true;
    editorControls();
    message($("editor-feedback"), "pending", "正在读取服务商的模型列表…");
    try {
      const result = await invoke("model_catalog", { input, client });
      setCandidates(result.models, "remote", result.partial);
      const count = candidates.filter(model => supportsCandidate(model.id)).length;
      message($("editor-feedback"), count ? "success" : "warning", count
        ? `已读取 ${count} 个模型。` : "服务商返回的列表没有可选模型。可手动填写模型 ID。");
    } catch (error) {
      const detail = errorText(error);
      const reason = currentPreset()?.id === "qwen" && detail.endsWith("（HTTP 401）")
        ? "账号目录鉴权失败（HTTP 401）。请确认使用千问 Token Plan 专属 API Key（以 sk-sp- 开头）。"
        : `未能读取账号目录：${detail}`;
      const fallback = source === "remote" ? "仍可搜索上次读取的目录或手动填写模型 ID。"
        : candidates.length ? "仍可搜索预置模型或手动填写模型 ID。" : "可手动填写模型 ID。";
      message($("editor-feedback"), "warning", `${reason} ${fallback}`);
    } finally {
      busy = false;
      if (fromConnection) step("selection");
      else {
        editorControls();
        $("refresh-catalog").focus({ preventScroll: true });
      }
    }
  }

  async function runModelAction(command) {
    if (busy || !editor) return;
    const testing = command === "test_model";
    if ($(testing ? "probe-model" : "save-model").disabled) return;
    if (!testing && !$("selected-models").hidden && !addManualModels()) return;
    const input = draft(testing);
    if (command === "save_model" && input.desktop_models?.some(isAuto)) {
      message($("editor-feedback"), "error", "Desktop 列表需要具体模型 ID。请移除 auto。");
      return;
    }
    const targetId = testing && editor.desktop && selectedModels().length ? "desktop" : editor.target;
    const selection = editor.id ? [] : selectedModels();
    const batch = command === "save_model" && selection.length > 1;
    const selected = selection.slice(1).map(id => ({ model: id, name: candidateName(id) }));
    const selectAfterSave = command === "save_model" && saveBeforeApplying();
    const agent = target()?.agent;
    const apply = command === "save_model" && !agent && !selectAfterSave && lite.canApply(target());
    busy = true;
    editorControls();
    message($("editor-feedback"), "pending", testing ? "正在发送一条简短测试请求…" : "正在保存模型…");
    try {
      if (testing) {
        const result = await invoke(command, { input, client: targetId || null });
        message($("editor-feedback"), result.healthy ? "success" : "error",
          result.healthy ? `${result.message}，耗时 ${result.latency_ms} 毫秒。` : result.message);
        if (input.id && !input.key.trim()) await lite.refresh();
      } else {
        const args = { input, client: targetId || null };
        const ids = batch ? await invoke("save_models", { ...args, selected }) : [await invoke(command, args)];
        const [id] = ids;
        await lite.refresh();
        busy = false;
        closeEditor();
        filters.saved = emptyFilters();
        setTab("saved");
        message($("models-feedback"), "success", batch ? `已配置 ${selection.length} 个模型。` : "模型已保存。");
        if (selectAfterSave) {
          lite.showSavedProfile(targetId, id, "档案已保存。选择连接方式后，点击「应用模式」。");
        } else if (agent) {
          lite.addAgentDraft(targetId, ids);
        } else if (apply) {
          lite.selectProfile(targetId, id);
          if (lite.page === "models") lite.showClient(targetId);
          if (batch && targetId === "workbuddy") await lite.clientAction(targetId, "apply_workbuddy_models", { ids }, true);
          else await lite.clientAction(targetId, "apply_model", { id }, true);
        } else if (lite.page === "models" && !editor) {
          $(`saved-model:${id}`)?.focus({ preventScroll: true });
        }
      }
    } catch (error) {
      message($("editor-feedback"), "error", errorText(error));
    } finally {
      busy = false;
      editorControls();
    }
  }

  window.addEventListener("lite:refresh", () => { syncFilters(); renderLibrary(); });
  window.addEventListener("lite:refresh-error", () => { if (!lite.data.presets.length) renderLibrary(); });
  window.addEventListener("lite:new-model", event => openEditor({ ...event.detail, fromClient: true }));
  $("new-model").addEventListener("click", event => openNewModel(event.currentTarget));
  $("close-model").addEventListener("click", () => closeEditor(true));
  $("provider-picker").addEventListener("change", () => {
    if (currentPreset()?.id !== editor.preset) editor.initialModel = "";
    syncAccounts();
    syncConnection({ resetOnProviderChange: true });
    message($("editor-feedback"), "");
  });
  $("account-picker").addEventListener("change", () => {
    if (!$("provider-picker").value && currentAccount()) $("provider-picker").value = providerId(currentAccount());
    if (source === "remote" && !editor.id) resetModelSelection();
    syncAccounts($("account-picker").value);
    syncConnection({ resetOnProviderChange: true });
    message($("editor-feedback"), "");
  });
  $("provider-protocol").addEventListener("change", () => {
    const bearer = $("provider-protocol").value !== "anthropic";
    $("auth-field").hidden = bearer;
    if (bearer) $("provider-auth").value = "auth_token";
    updateModelOptions();
    setCandidates(currentPreset()?.models || [], "preset");
    editorControls();
  });
  desktopList.addEventListener("input", () => { message($("editor-feedback"), ""); editorControls(); });
  $("add-desktop-candidate").addEventListener("click", () => {
    desktopList.value = mergeModelIds(modelLines(desktopList.value), selectedModels().filter(id => !isAuto(id))).join("\n");
    editorControls();
  });
  $("add-manual-model").addEventListener("click", () => {
    if (addManualModels()) manualIdsInput.focus();
  });
  manualIdsInput.addEventListener("input", () => {
    $("add-manual-model").disabled = !manualIdsInput.value.trim();
    message($("editor-feedback"), "");
  });
  modelIdInput.addEventListener("input", () => { message($("editor-feedback"), ""); editorControls(); });
  for (const id of ["provider-key", "provider-url", "provider-auth"]) {
    $(id).addEventListener("input", () => {
      if (id === "provider-url" || id === "provider-auth") $("provider-key").value = "";
      if (source === "remote") {
        if (!editor.id) resetModelSelection();
        setCandidates(currentPreset()?.models || [], "preset");
      } else {
        renderCandidates();
      }
      message($("editor-feedback"), "");
      editorControls();
    });
  }
  $("candidate-search").addEventListener("input", refreshCandidateResults);
  $("candidate-search").addEventListener("keydown", event => {
    if (event.key === "Enter" && !event.isComposing) event.preventDefault();
  });
  $("model-search").addEventListener("input", event => changeFilter("query", event.currentTarget.value));
  $("library-client").addEventListener("change", event => changeFilter("target", event.currentTarget.value));
  $("library-provider").addEventListener("change", event => changeFilter("provider", event.currentTarget.value));
  $("clear-library-filters").addEventListener("click", () => {
    filters[tab] = emptyFilters();
    message($("models-feedback"), "");
    setTab(tab);
    $("model-search").focus();
  });
  for (const control of modelTabs) {
    control.addEventListener("click", () => { message($("models-feedback"), ""); setTab(control.dataset.modelTab); });
  }
  $("skip-catalog").addEventListener("click", () => step("selection"));
  $("back-to-connection").addEventListener("click", () => step("connection"));
  $("fetch-models").addEventListener("click", loadCatalog);
  $("refresh-catalog").addEventListener("click", loadCatalog);
  $("probe-model").addEventListener("click", () => runModelAction("test_model"));
  form.addEventListener("submit", event => {
    event.preventDefault();
    if (!$("selection-step").hidden) void runModelAction("save_model");
  });
  syncFilters();
  renderLibrary();
})();

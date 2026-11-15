"use strict";

(() => {
  const { $, invoke, message, errorText } = lite;
  const form = $("configure");
  const input = $("mcp-key");
  const button = $("configure-button");
  const badge = $("state");
  const feedback = $("feedback");
  const target = $("target");
  const modes = { native: "官方模式", gateway: "第三方模型模式", custom: "自定义资料目录" };
  const states = { unconfigured: "未接入", configured: "已接入", existing: "已有配置", needs_repair: "需要重配", permission_managed: "由第三方档案管理" };
  let busy = false;
  let permissionManaged = false;
  let sequence = 0;

  function showStatus({ state, mode }) {
    permissionManaged = state === "permission_managed";
    badge.dataset.state = state;
    badge.textContent = states[state] || "待检查";
    target.textContent = permissionManaged
      ? "如由 Lite 设置了工具权限，请在「已配置 → FrameLeap → 工具权限」清空授权后更新 Key。"
      : `写入 Claude Desktop 的${modes[mode] || "当前配置"}。接入后完全退出并重新打开客户端。`;
    controls();
  }

  function controls() {
    button.disabled = !invoke || busy || permissionManaged || !input.value.trim();
    input.disabled = !invoke || busy || permissionManaged;
    button.textContent = busy ? "正在验证…" : "验证并接入";
  }

  input.addEventListener("input", controls);
  $("frameleap-setup").addEventListener("toggle", () => {
    if (!$("frameleap-setup").open && !busy) {
      input.value = "";
      controls();
    }
  });
  form.addEventListener("submit", async event => {
    event.preventDefault();
    if (!invoke || busy || permissionManaged || !input.value.trim()) return;
    const key = input.value.trim();
    sequence += 1;
    busy = true;
    form.setAttribute("aria-busy", "true");
    controls();
    message(feedback, "pending", "正在验证 Key 并读取 FrameLeap 工具列表…");
    try {
      const result = await invoke("configure_frameleap", { key });
      input.value = "";
      showStatus({ state: "configured", mode: result.mode });
      message(feedback, "success", `已保存配置并验证 ${result.tool_count} 个工具。`);
      window.dispatchEvent(new Event("lite:frameleap-updated"));
    } catch (error) {
      message(feedback, "error", errorText(error));
    } finally {
      busy = false;
      form.removeAttribute("aria-busy");
      controls();
    }
  });

  window.refreshFrameLeap = async function () {
    if (!invoke || busy) return;
    const request = ++sequence;
    try {
      const status = await invoke("frameleap_status");
      if (request === sequence) showStatus(status);
    } catch (error) {
      if (request !== sequence) return;
      badge.dataset.state = "unavailable";
      badge.textContent = "待检查";
      if (feedback.dataset.kind !== "success") message(feedback, "error", errorText(error));
    }
  };
  window.clearFrameLeapFeedback = () => message(feedback, "");
  controls();
})();

#!/usr/bin/env python3
"""Verify the actual .app (including one mounted from a DMG) before delivery."""
import hashlib
import json
import pathlib
import plistlib
import subprocess
import sys

root = pathlib.Path(__file__).resolve().parents[1]
if len(sys.argv) != 2:
    raise SystemExit("用法：npm run verify:macos -- /path/to/codeneo-lite.app")
app = pathlib.Path(sys.argv[1]).resolve()
expected = json.loads((root / "package.json").read_text())["version"]
info = plistlib.loads((app / "Contents/Info.plist").read_bytes())
for field in ("CFBundleShortVersionString", "CFBundleVersion"):
    if info.get(field) != expected:
        raise SystemExit(f"安装包版本不一致：{field}={info.get(field)}，源码版本={expected}")
binary = app / "Contents/MacOS" / info["CFBundleExecutable"]
subprocess.run(["codesign", "--verify", "--deep", "--strict", "--all-architectures", str(app)], check=True)
architectures = subprocess.check_output(["lipo", "-archs", str(binary)], text=True).split()
if set(architectures) != {"arm64", "x86_64"}:
    raise SystemExit(f"安装包缺少通用架构：{architectures}")
# These builds predate the command; never accidentally start their GUI/gateway.
if b"--check-package" not in binary.read_bytes():
    raise SystemExit("安装包不含计价自检入口，疑似旧构建")
report = json.loads(subprocess.check_output([str(binary), "--check-package"], text=True, timeout=20))
if report.get("version") != expected or report.get("pricing_contract") != 2:
    raise SystemExit(f"可执行文件版本或计价接口与安装包不一致：{report}")
if report.get("qwen4_aliases") != "plan_only" or report.get("unknown_model") != "missing_price":
    raise SystemExit(f"计价状态验证失败：{report}")
if abs(report.get("glm_5_3_cny", -1) - 0.0146) > 1e-10:
    raise SystemExit(f"GLM 计价验证失败：{report}")
if abs(report.get("qwen4_max_cny", -1) - 0.0185) > 1e-10:
    raise SystemExit(f"Qwen4-Max 计价验证失败：{report}")
report.update(app=str(app), architectures=architectures, binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest())
print(json.dumps(report, ensure_ascii=False, indent=2))

# CodeNeo Lite 图标

`app.svg` 在原 CodeNeo 图标上叠加 LITE 徽标。`tray.svg` 是 macOS 菜单栏使用的单色图标，系统会根据外观着色。

在仓库根目录执行 `npm ci` 后，可以重新生成打包和界面使用的图标：

```sh
icons_tmp="$(mktemp -d)"
npm exec -- tauri icon src-tauri/icons/source/app.svg --output "$icons_tmp/app"
npm exec -- tauri icon src-tauri/icons/source/tray.svg --png 144 --output "$icons_tmp/tray"
cp "$icons_tmp/app/icon.png" "$icons_tmp/app/icon.icns" "$icons_tmp/app/icon.ico" src-tauri/icons/
cp "$icons_tmp/app/64x64.png" ui/icon.png
cp "$icons_tmp/tray/144x144.png" src-tauri/icons/trayTemplate.png
```

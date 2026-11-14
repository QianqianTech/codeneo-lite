use super::{
    entry, name_valid, options,
    targets::{self, Scope},
    transports, Connection, Input, Transport,
};
use crate::{agents, files, launch, secrets, transaction::Client};
use claudekeeper_claude::is_launchable_file;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::LazyLock,
};

const SOURCE: &str = include_str!("../../../mcp/catalog.json");

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Catalog {
    #[serde(rename = "$schema")]
    _schema: Option<String>,
    version: u32,
    servers: Vec<Preset>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Preset {
    id: String,
    name: String,
    category: String,
    description: String,
    source: String,
    variants: Vec<Variant>,
}

#[derive(Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum InputTarget {
    Header,
    Env,
    Directory,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Prompt {
    target: InputTarget,
    label: String,
    required: bool,
    #[serde(default)]
    key: String,
    #[serde(default)]
    prefix: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Variant {
    #[serde(default)]
    clients: Vec<Client>,
    transport: Transport,
    endpoint: String,
    #[serde(default)]
    args: Vec<String>,
    #[serde(default)]
    options: BTreeMap<String, String>,
    #[serde(default)]
    builtin: Option<String>,
    input: Option<Prompt>,
    #[serde(default)]
    oauth: bool,
    requires: Option<String>,
    note: Option<String>,
}

#[derive(Serialize)]
struct Field<'a> {
    label: &'a str,
    required: bool,
    directory: bool,
    project_variable: bool,
}

#[derive(Serialize)]
pub struct Card<'a> {
    id: &'a str,
    name: &'a str,
    category: &'a str,
    description: &'a str,
    transport: Transport,
    endpoint: &'a str,
    args: &'a [String],
    input: Option<Field<'a>>,
    oauth: bool,
    requires: Option<&'a str>,
    note: Option<&'a str>,
}

static CATALOG: LazyLock<Result<Catalog, String>> = LazyLock::new(|| {
    let catalog: Catalog = serde_json::from_str(SOURCE).map_err(|_| "内置 MCP 目录格式无效")?;
    if catalog.version != 1 {
        return Err("此版本的 Lite 无法读取内置 MCP 目录".into());
    }
    Ok(catalog)
});

fn presets() -> Result<&'static [Preset], String> {
    CATALOG
        .as_ref()
        .map(|catalog| catalog.servers.as_slice())
        .map_err(Clone::clone)
}

fn preset(id: &str) -> Result<&'static Preset, String> {
    presets()?
        .iter()
        .find(|preset| preset.id == id)
        .ok_or_else(|| "找不到此 MCP 目录条目".into())
}

impl Variant {
    fn supports(&self, client: Client) -> bool {
        transports(client).contains(&self.transport)
            && (self.clients.is_empty() || self.clients.contains(&client))
    }
}

impl Preset {
    fn variant(&self, client: Client) -> Option<&Variant> {
        self.variants
            .iter()
            .find(|variant| variant.supports(client))
    }
}

pub fn list(client: Client) -> Result<Vec<Card<'static>>, String> {
    Ok(presets()?
        .iter()
        .filter_map(|preset| {
            let variant = preset.variant(client)?;
            Some(Card {
                id: &preset.id,
                name: &preset.name,
                category: &preset.category,
                description: &preset.description,
                transport: variant.transport,
                endpoint: &variant.endpoint,
                args: &variant.args,
                input: variant.input.as_ref().map(|input| Field {
                    label: &input.label,
                    required: input.required,
                    directory: input.target == InputTarget::Directory,
                    project_variable: !matches!(
                        project_credential(client, input),
                        ProjectCredential::Unsupported
                    ),
                }),
                oauth: variant.oauth,
                requires: variant.requires.as_deref(),
                note: variant.note.as_deref(),
            })
        })
        .collect())
}

pub fn source(id: &str) -> Result<&'static str, String> {
    let url = &preset(id)?.source;
    if super::checked_url(url.split('#').next().unwrap_or_default())?.scheme() != "https" {
        return Err("目录条目的来源需要 HTTPS 链接".into());
    }
    Ok(url)
}

pub fn prepare(
    client: Client,
    scope: Scope,
    id: &str,
    value: Option<&str>,
    resources: &Path,
) -> Result<Input, String> {
    let variant = preset(id)?
        .variant(client)
        .ok_or("所选客户端不支持此 MCP 的连接方式")?;
    let mut input = build(client, scope, id, variant, value)?;
    if let Some(builtin) = &variant.builtin {
        let entry = builtin_entry(resources, builtin, &secrets::root()?)?;
        input
            .connection
            .args
            .insert(0, entry.to_str().ok_or("内置 MCP 路径编码无效")?.into());
    }
    if variant.transport == Transport::Stdio {
        resolve_launcher(&mut input.connection, &agents::home()?, scope)?;
    }
    // Check the recipe with the same validation as a manual save.
    entry(client, &input.connection)?;
    Ok(input)
}

fn builtin_entry(resources: &Path, builtin: &str, data: &Path) -> Result<PathBuf, String> {
    let name = match builtin {
        "playwright" => "playwright",
        _ => return Err("内置 MCP 目录条目无效".into()),
    };
    let source = resources.join("mcp/playwright/node_modules");
    let entry = Path::new("@playwright/mcp/cli.js");
    let runtime = data
        .join("mcp")
        .join(format!("{name}-{}", env!("CARGO_PKG_VERSION")));
    files::private_dir(&runtime)?;
    let installed = runtime.join("node_modules");
    let installed_entry = installed.join(entry);
    if installed_entry.is_file() {
        return Ok(installed_entry);
    }
    if !source.join(entry).is_file() {
        return Err("内置 Playwright MCP 文件缺失，请重新安装 Lite".into());
    }

    // AppImage resources are temporary mounts. Stage an offline copy beside the
    // stable destination so saved client configs never reference that mount.
    let staging = tempfile::Builder::new()
        .prefix(".install-")
        .tempdir_in(&runtime)
        .map_err(|_| "无法暂存内置 Playwright MCP")?;
    let staged = staging.path().join("node_modules");
    copy_builtin(&source, &staged)?;
    if let Err(error) = std::fs::rename(&staged, &installed) {
        if !installed_entry.is_file() {
            return Err(match error.kind() {
                std::io::ErrorKind::AlreadyExists => {
                    "内置 Playwright MCP 的安装目录无效，请重新安装 Lite".into()
                }
                _ => "无法准备内置 Playwright MCP".into(),
            });
        }
    }
    Ok(installed_entry)
}

fn copy_builtin(source: &Path, destination: &Path) -> Result<(), String> {
    std::fs::create_dir(destination).map_err(|_| "无法创建内置 MCP 安装目录")?;
    for entry in std::fs::read_dir(source).map_err(|_| "无法读取内置 MCP 资源")? {
        let entry = entry.map_err(|_| "无法读取内置 MCP 资源")?;
        let name = entry.file_name();
        if name == ".bin" {
            continue;
        }
        let kind = entry
            .file_type()
            .map_err(|_| "无法检查内置 MCP 资源")?;
        let target = destination.join(name);
        if kind.is_dir() {
            copy_builtin(&entry.path(), &target)?;
        } else if kind.is_file() {
            std::fs::copy(entry.path(), target).map_err(|_| "无法复制内置 MCP 资源")?;
        } else {
            return Err("内置 MCP 资源包含不支持的链接".into());
        }
    }
    Ok(())
}

pub fn legacy_playwright_migration(
    client: Client,
    scope: Scope,
    name: &str,
    server: &serde_json::Value,
    resources: &Path,
) -> Result<Option<Input>, String> {
    if name != "playwright" {
        return Ok(None);
    }
    let before = super::connection(client, server)?;
    let Some(args) = legacy_playwright_args(&before) else {
        return Ok(None);
    };

    let mut input = prepare(client, scope, name, None, resources)?;
    let entry = input.connection.args.remove(0);
    input.connection.args = std::iter::once(entry).chain(args).collect();
    input.connection.options = before.options;
    Ok(Some(input))
}

fn legacy_playwright_args(connection: &Connection) -> Option<Vec<String>> {
    if connection.transport != Transport::Stdio {
        return None;
    }
    let launcher = Path::new(&connection.endpoint)
        .file_stem()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let mut args = connection.args.clone();
    let npm_script = args.first().is_some_and(|arg| {
        Path::new(arg)
            .file_name()
            .is_some_and(|name| name == "npx-cli.js")
    });
    if !launcher.eq_ignore_ascii_case("npx") && !npm_script {
        return None;
    }
    if npm_script {
        args.remove(0);
    }
    if args.first().is_some_and(|arg| arg == "-y") {
        args.remove(0);
    }
    let package = args.first().map(String::as_str).unwrap_or_default();
    if package != "@playwright/mcp"
        && !package
            .strip_prefix("@playwright/mcp@")
            .is_some_and(|version| !version.is_empty())
    {
        return None;
    }
    args.remove(0);
    Some(args)
}

fn resolve_launcher(connection: &mut Connection, home: &Path, scope: Scope) -> Result<(), String> {
    if scope == Scope::Project && !Path::new(&connection.endpoint).is_absolute() {
        // Keep the shared config usable on teammates' machines.
        executable(&connection.endpoint, home)?;
        return Ok(());
    }
    let command = if connection.endpoint == "npx" {
        let npx = executable("npx", home)?;
        let node = executable("node", home)?;
        if let Some(script) = npx_script(&npx, &node) {
            // The npm shim uses /usr/bin/env node, and GUI clients may not have
            // the shell's PATH. Point them directly to Node and the npm script.
            connection
                .args
                .insert(0, script.to_str().ok_or("npx 路径编码无效")?.into());
            if !connection.options.contains_key("PATH") {
                let path = launch::agent_path(Some(&node), home)?;
                connection.options.insert(
                    "PATH".into(),
                    path.into_string().map_err(|_| "Node 搜索路径编码无效")?,
                );
            }
            node
        } else {
            npx
        }
    } else {
        executable(&connection.endpoint, home)?
    };
    connection.endpoint = command.to_str().ok_or("MCP 启动程序路径编码无效")?.into();
    Ok(())
}

fn npx_script(npx: &Path, node: &Path) -> Option<PathBuf> {
    let resolved = dunce::canonicalize(npx).ok()?;
    if resolved
        .file_name()
        .is_some_and(|name| name == "npx-cli.js")
    {
        return Some(resolved);
    }
    let beside_node = node.parent()?.join("node_modules/npm/bin/npx-cli.js");
    beside_node.is_file().then_some(beside_node)
}

enum ProjectCredential {
    Field(&'static str),
    Expand,
    Unsupported,
}

fn project_credential(client: Client, prompt: &Prompt) -> ProjectCredential {
    match client {
        Client::Code | Client::Qwen | Client::Codebuddy => ProjectCredential::Expand,
        Client::Codex | Client::Kimi
            if prompt.target == InputTarget::Header
                && prompt.key.eq_ignore_ascii_case("authorization")
                && prompt.prefix == "Bearer " =>
        {
            ProjectCredential::Field(if client == Client::Codex {
                "bearer_token_env_var"
            } else {
                "bearerTokenEnvVar"
            })
        }
        _ => ProjectCredential::Unsupported,
    }
}

fn build(
    client: Client,
    scope: Scope,
    id: &str,
    variant: &Variant,
    value: Option<&str>,
) -> Result<Input, String> {
    if !name_valid(id) {
        return Err("MCP 目录条目的服务名称无效".into());
    }
    let mut args = variant.args.clone();
    let mut options = variant.options.clone();
    let mut advanced = BTreeMap::new();
    let value = value.filter(|value| !value.trim().is_empty());
    match (&variant.input, value) {
        (Some(prompt), Some(value)) => match prompt.target {
            InputTarget::Directory if variant.transport == Transport::Stdio => {
                let path = targets::directory(Path::new(value))?;
                args.push(path.to_str().ok_or("目录路径编码无效")?.into());
            }
            InputTarget::Header if variant.transport != Transport::Stdio => {
                add_credential(
                    client,
                    scope,
                    prompt,
                    value.trim(),
                    &mut options,
                    &mut advanced,
                )?;
            }
            InputTarget::Env if variant.transport == Transport::Stdio => {
                add_credential(
                    client,
                    scope,
                    prompt,
                    value.trim(),
                    &mut options,
                    &mut advanced,
                )?;
            }
            _ => return Err("MCP 目录条目的输入类型与连接方式不匹配".into()),
        },
        (Some(prompt), None) if prompt.required => {
            return Err(format!("请填写「{}」", prompt.label))
        }
        (None, Some(_)) => return Err("此服务不需要填写连接参数".into()),
        _ => {}
    }
    Ok(Input {
        name: id.into(),
        connection: Connection {
            transport: variant.transport,
            endpoint: variant.endpoint.clone(),
            args,
            options,
        },
        revision: None,
        advanced,
    })
}

fn add_credential(
    client: Client,
    scope: Scope,
    prompt: &Prompt,
    value: &str,
    static_options: &mut BTreeMap<String, String>,
    advanced: &mut BTreeMap<String, Option<serde_json::Value>>,
) -> Result<(), String> {
    if scope != Scope::Project {
        static_options.insert(prompt.key.clone(), format!("{}{}", prompt.prefix, value));
    } else {
        if !options::variable_name(value) {
            return Err("请填写环境变量名（例如 MCP_TOKEN），不要粘贴 Key 或令牌值".into());
        }
        match project_credential(client, prompt) {
            ProjectCredential::Field(key) => {
                advanced.insert(key.into(), Some(serde_json::json!(value)));
            }
            ProjectCredential::Expand => {
                static_options.insert(prompt.key.clone(), format!("{}${{{value}}}", prompt.prefix));
            }
            ProjectCredential::Unsupported => {
                return Err("此连接在项目共享范围不能使用该凭据，请切换到用户级配置".into())
            }
        }
    }
    Ok(())
}

fn executable(command: &str, home: &Path) -> Result<PathBuf, String> {
    let command_path = Path::new(command);
    if command_path.is_absolute() && is_launchable_file(command_path) {
        return Ok(command_path.into());
    }
    let path = launch::agent_path(None, home)?;
    let dirs = std::env::split_paths(&path);
    #[cfg(windows)]
    let dirs = dirs.chain(
        std::env::var_os("ProgramFiles")
            .into_iter()
            .flat_map(|base| {
                ["nodejs", "Docker/Docker/resources/bin"].map(|dir| PathBuf::from(&base).join(dir))
            })
            .chain(
                std::env::var_os("LOCALAPPDATA")
                    .map(|base| PathBuf::from(base).join("Programs/nodejs")),
            ),
    );
    let names = if cfg!(windows) && command_path.extension().is_none() {
        vec![
            format!("{command}.exe"),
            format!("{command}.cmd"),
            command.to_owned(),
        ]
    } else {
        vec![command.to_owned()]
    };
    if command_path.components().count() == 1 {
        for dir in dirs {
            for name in &names {
                let path = dir.join(name);
                if is_launchable_file(&path) {
                    return Ok(path);
                }
            }
        }
    }
    Err(format!("未找到启动程序 {command}。请安装所需程序后重开 Lite，或在「添加 MCP」中填写可执行文件的完整路径。"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn finds_the_npm_script_in_a_windows_node_distribution() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let shim = root.join("npx.cmd");
        let node = root.join("node.exe");
        let script = root.join("node_modules/npm/bin/npx-cli.js");
        std::fs::create_dir_all(script.parent().unwrap()).unwrap();
        std::fs::write(&script, "fixture").unwrap();
        std::fs::write(&shim, "fixture").unwrap();
        assert_eq!(npx_script(&shim, &node), Some(script));
    }

    #[test]
    fn bundled_playwright_entry_is_copied_to_a_stable_directory() {
        let dir = tempfile::tempdir().unwrap();
        let resources = dir.path().join("resources");
        let data = dir.path().join("data");
        let relative = "mcp/playwright/node_modules/@playwright/mcp/cli.js";
        let source = resources.join(relative);
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        std::fs::write(&source, "// fixture").unwrap();

        let installed = builtin_entry(&resources, "playwright", &data).unwrap();
        assert!(installed.starts_with(&data));
        assert_eq!(std::fs::read_to_string(&installed).unwrap(), "// fixture");

        std::fs::remove_file(source).unwrap();
        assert_eq!(
            builtin_entry(&resources, "playwright", &data).unwrap(),
            installed
        );

        let missing = tempfile::tempdir().unwrap();
        let error = builtin_entry(missing.path(), "playwright", &data.join("missing"))
            .unwrap_err();
        assert!(error.contains("文件缺失"));
    }

    #[test]
    fn legacy_playwright_launchers_are_detected_without_touching_custom_servers() {
        let connection = |endpoint: &str, args: &[&str]| Connection {
            transport: Transport::Stdio,
            endpoint: endpoint.into(),
            args: args.iter().map(|arg| (*arg).into()).collect(),
            options: BTreeMap::new(),
        };
        let npx = connection(
            "/usr/local/bin/npx",
            &["-y", "@playwright/mcp@latest", "--isolated"],
        );
        assert_eq!(legacy_playwright_args(&npx), Some(vec!["--isolated".into()]));

        let npm_script = connection(
            "/usr/local/bin/node",
            &[
                "/usr/local/lib/node_modules/npm/bin/npx-cli.js",
                "-y",
                "@playwright/mcp@0.0.36",
                "--headless",
            ],
        );
        assert_eq!(
            legacy_playwright_args(&npm_script),
            Some(vec!["--headless".into()])
        );

        let custom = connection(
            "/usr/local/bin/node",
            &["/custom/server.js", "@playwright/mcp@latest"],
        );
        assert_eq!(legacy_playwright_args(&custom), None);
    }

    #[test]
    fn catalog_variants_use_supported_native_formats() {
        let dir = tempfile::tempdir().unwrap();
        let mut ids = BTreeSet::new();
        for preset in presets().unwrap() {
            assert!(preset
                .id
                .bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-'));
            assert!(ids.insert(&preset.id));
            assert!(source(&preset.id).unwrap().starts_with("https://"));
            assert!(!preset.variants.is_empty());
            for variant in &preset.variants {
                assert!(Client::ALL.iter().any(|&client| variant.supports(client)));
                assert!(variant
                    .clients
                    .iter()
                    .all(|&client| transports(client).contains(&variant.transport)));
                let value = variant.input.as_ref().map(|field| {
                    if field.target == InputTarget::Directory {
                        dir.path().to_str().unwrap()
                    } else {
                        "fixture-value"
                    }
                });
                for client in Client::ALL
                    .into_iter()
                    .filter(|&client| variant.supports(client))
                {
                    let input = build(client, Scope::User, &preset.id, variant, value).unwrap();
                    let native = entry(client, &input.connection).unwrap();
                    assert!(Transport::from_entry(client, &native) == Some(variant.transport));
                }
                if variant.input.as_ref().is_some_and(|field| field.required) {
                    assert!(build(Client::Codex, Scope::User, &preset.id, variant, None).is_err());
                }
            }
        }
        assert_eq!(
            preset("context7")
                .unwrap()
                .variant(Client::Desktop)
                .unwrap()
                .endpoint,
            "npx"
        );
        assert_eq!(
            preset("context7")
                .unwrap()
                .variant(Client::Codex)
                .unwrap()
                .endpoint,
            "https://mcp.context7.com/mcp"
        );
        let github = preset("github").unwrap().variant(Client::Codex).unwrap();
        let input = build(
            Client::Codex,
            Scope::User,
            "github",
            github,
            Some("fixture"),
        )
        .unwrap();
        assert_eq!(input.connection.options["Authorization"], "Bearer fixture");
        assert_eq!(input.connection.options["X-MCP-Readonly"], "true");
    }

    #[test]
    fn project_recipes_reference_environment_variables_without_writing_token_values() {
        let github = preset("github").unwrap();
        for (client, field) in [
            (Client::Codex, "bearer_token_env_var"),
            (Client::Kimi, "bearerTokenEnvVar"),
        ] {
            let variant = github.variant(client).unwrap();
            let input = build(
                client,
                Scope::Project,
                "github",
                variant,
                Some("GITHUB_MCP_TOKEN"),
            )
            .unwrap();
            assert!(!input.connection.options.contains_key("Authorization"));
            let native =
                super::super::prepared_entry(Path::new("fixture"), client, &input, None).unwrap();
            assert_eq!(native[field], "GITHUB_MCP_TOKEN");
            assert!(build(
                client,
                Scope::Project,
                "github",
                variant,
                Some("ghp-fixture-token")
            )
            .is_err());
        }
        let variant = github.variant(Client::Code).unwrap();
        let input = build(
            Client::Code,
            Scope::Project,
            "github",
            variant,
            Some("GITHUB_MCP_TOKEN"),
        )
        .unwrap();
        assert_eq!(
            input.connection.options["Authorization"],
            "Bearer ${GITHUB_MCP_TOKEN}"
        );
    }
}

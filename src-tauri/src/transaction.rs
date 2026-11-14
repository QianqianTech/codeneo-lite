use crate::{files, vault};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};
use toml_edit::{DocumentMut, Item, Table, TableLike};

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum Client {
    Desktop,
    Code,
    Codex,
    Workbuddy,
    Opencode,
    Qwen,
    Codebuddy,
    Kimi,
    Copilot,
}

impl Client {
    pub const ALL: [Self; 9] = [
        Self::Desktop,
        Self::Code,
        Self::Codex,
        Self::Workbuddy,
        Self::Opencode,
        Self::Qwen,
        Self::Codebuddy,
        Self::Kimi,
        Self::Copilot,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
            Self::Code => "code",
            Self::Codex => "codex",
            Self::Workbuddy => "workbuddy",
            Self::Opencode => "opencode",
            Self::Qwen => "qwen",
            Self::Codebuddy => "codebuddy",
            Self::Kimi => "kimi",
            Self::Copilot => "copilot",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Desktop => "Claude Desktop",
            Self::Code => "Claude Code",
            Self::Codex => "Codex",
            Self::Workbuddy => "WorkBuddy",
            Self::Opencode => "OpenCode",
            Self::Qwen => "Qwen Code",
            Self::Codebuddy => "CodeBuddy Code CLI",
            Self::Kimi => "Kimi Code CLI",
            Self::Copilot => "GitHub Copilot CLI",
        }
    }

    fn backup(self, root: &Path) -> PathBuf {
        root.join(format!("previous-{}.bin", self.id()))
    }
}

#[derive(Clone, Deserialize, Serialize)]
struct Edit {
    section: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    nested: Vec<String>,
    key: String,
    before: Field,
    after: Field,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(tag = "state", content = "value")]
pub(crate) enum Field {
    Missing,
    Present(Value),
}

impl Field {
    pub(crate) fn value(&self) -> Option<&Value> {
        match self {
            Self::Missing => None,
            Self::Present(value) => Some(value),
        }
    }
}

impl From<Option<Value>> for Field {
    fn from(value: Option<Value>) -> Self {
        value.map(Self::Present).unwrap_or(Self::Missing)
    }
}

#[derive(Clone, Deserialize, Serialize)]
pub struct Delta {
    path: PathBuf,
    edits: Vec<Edit>,
    #[serde(default)]
    jsonc: bool,
}

impl Delta {
    pub(crate) fn at(&self, path: &Path) -> bool {
        self.path == path
    }
}

#[derive(Deserialize, Serialize)]
struct Journal {
    id: String,
    client: Client,
    deltas: Vec<Delta>,
}

pub fn decode(data: Option<&[u8]>) -> Result<Map<String, Value>, String> {
    let Some(data) = data else {
        return Ok(Map::new());
    };
    serde_json::from_slice(data.strip_prefix(b"\xef\xbb\xbf").unwrap_or(data))
        .map_err(|_| "配置文件必须是有效的 JSON 对象".into())
}

pub fn json_bytes<T: Serialize>(value: &T) -> Result<Vec<u8>, String> {
    let mut bytes = serde_json::to_vec_pretty(value).map_err(|_| "无法生成本机配置")?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn toml_text(bytes: Option<&[u8]>) -> Result<&str, String> {
    std::str::from_utf8(bytes.unwrap_or_default())
        .map(|text| text.trim_start_matches('\u{feff}'))
        .map_err(|_| "Codex 配置必须使用 UTF-8".into())
}

fn is_toml(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension == "toml")
}

fn read_document(
    path: &Path,
    bytes: Option<&[u8]>,
    jsonc: bool,
) -> Result<Map<String, Value>, String> {
    if is_toml(path) {
        return toml_edit::de::from_str(toml_text(bytes)?)
            .map_err(|_| "本机 TOML 配置格式无效，请先在客户端中修复".into());
    }
    if jsonc {
        return crate::jsonc::document(bytes)?.decode();
    }
    decode(bytes)
}

fn edit_toml(
    doc: &mut DocumentMut,
    sections: &[&str],
    key: &str,
    value: Option<&Value>,
) -> Result<(), String> {
    let mut table: &mut dyn TableLike = doc.as_table_mut();
    for &section in sections {
        if value.is_none() && !table.contains_key(section) {
            return Ok(());
        }
        table = table
            .entry(section)
            .or_insert_with(|| Item::Table(Table::new()))
            .as_table_like_mut()
            .ok_or("TOML 配置中的表格式无效")?;
    }
    if let Some(value) = value {
        let mut serialized =
            toml_edit::ser::to_document(&Map::from_iter([(key.to_string(), value.clone())]))
                .map_err(|_| "无法生成 Codex 配置字段")?;
        let item = serialized
            .as_table_mut()
            .remove(key)
            .ok_or("Codex 配置字段为空")?;
        if let Some(existing) = table.get_mut(key) {
            if let Some(previous) = existing.as_value() {
                let mut value = item.into_value().map_err(|_| "Codex 配置值格式无效")?;
                *value.decor_mut() = previous.decor().clone();
                *existing = Item::Value(value);
            } else {
                *existing = item;
            }
        } else {
            table.insert(key, item);
        }
    } else {
        table.remove(key);
    }
    Ok(())
}

fn section<'a>(
    mut doc: &'a Map<String, Value>,
    names: &[&str],
) -> Result<Option<&'a Map<String, Value>>, String> {
    for &name in names {
        let Some(value) = doc.get(name) else {
            return Ok(None);
        };
        doc = value.as_object().ok_or("配置中的对象格式无效")?;
    }
    Ok(Some(doc))
}

fn set(
    mut doc: &mut Map<String, Value>,
    names: &[&str],
    key: &str,
    value: Option<Value>,
) -> Result<(), String> {
    for &name in names {
        if value.is_none() && !doc.contains_key(name) {
            return Ok(());
        }
        doc = doc
            .entry(name)
            .or_insert_with(|| Value::Object(Map::new()))
            .as_object_mut()
            .ok_or("配置中的对象格式无效")?;
    }
    if let Some(value) = value {
        doc.insert(key.to_string(), value);
    } else {
        doc.remove(key);
    }
    Ok(())
}

pub struct Plan {
    doc: Map<String, Value>,
    delta: Delta,
}

impl Plan {
    pub fn open(path: &Path) -> Result<Self, String> {
        Self::open_as(path, path.extension().is_some_and(|ext| ext == "jsonc"))
    }
    pub fn open_as(path: &Path, jsonc: bool) -> Result<Self, String> {
        let bytes = files::read(path)?;
        Ok(Self {
            doc: read_document(path, bytes.as_deref(), jsonc)?,
            delta: Delta {
                path: path.to_path_buf(),
                edits: Vec::new(),
                jsonc,
            },
        })
    }
    pub fn get(&self, name: Option<&str>, key: &str) -> Result<Option<&Value>, String> {
        self.get_at(name.as_slice(), key)
    }
    pub fn get_at(&self, names: &[&str], key: &str) -> Result<Option<&Value>, String> {
        Ok(section(&self.doc, names)?.and_then(|map| map.get(key)))
    }
    pub fn set(
        &mut self,
        name: Option<&str>,
        key: &str,
        after: Option<Value>,
    ) -> Result<(), String> {
        self.set_at(name.as_slice(), key, after)
    }
    pub fn set_at(
        &mut self,
        names: &[&str],
        key: &str,
        after: Option<Value>,
    ) -> Result<(), String> {
        let before = self.get_at(names, key)?.cloned();
        if before != after {
            set(&mut self.doc, names, key, after.clone())?;
            let mut path = names.iter().copied();
            let section = path.next().map(str::to_owned);
            let nested: Vec<_> = path.map(str::to_owned).collect();
            if let Some(index) = self.delta.edits.iter().position(|edit| {
                edit.section == section && edit.nested == nested && edit.key == key
            }) {
                let edit = &mut self.delta.edits[index];
                if edit.before.value() == after.as_ref() {
                    self.delta.edits.remove(index);
                } else {
                    edit.after = after.into();
                }
                return Ok(());
            }
            self.delta.edits.push(Edit {
                section,
                nested,
                key: key.into(),
                before: before.into(),
                after: after.into(),
            });
        }
        Ok(())
    }
    pub fn finish(self) -> Option<Delta> {
        (!self.delta.edits.is_empty()).then_some(self.delta)
    }
}

fn validate(journal: &Journal, paths: &[PathBuf]) -> Result<(), String> {
    if journal
        .deltas
        .iter()
        .any(|change| !paths.contains(&change.path))
    {
        return Err("Claude 的资料目录已改变，无法使用之前的切换记录".into());
    }
    Ok(())
}

fn apply(delta: &Delta, undo: bool) -> Result<(), String> {
    let bytes = files::read(&delta.path)?;
    let mut doc = read_document(&delta.path, bytes.as_deref(), delta.jsonc)?;
    let mut jsonc = (delta.jsonc && !is_toml(&delta.path))
        .then(|| crate::jsonc::document(bytes.as_deref()))
        .transpose()?;
    let mut toml = if is_toml(&delta.path) {
        Some(
            toml_text(bytes.as_deref())?
                .parse::<DocumentMut>()
                .map_err(|_| "无法读取 Codex 配置")?,
        )
    } else {
        None
    };
    let mut changed = false;
    for edit in &delta.edits {
        let names: Vec<_> = edit
            .section
            .as_deref()
            .into_iter()
            .chain(edit.nested.iter().map(String::as_str))
            .collect();
        let (from, to) = if undo {
            (edit.after.value(), edit.before.value())
        } else {
            (edit.before.value(), edit.after.value())
        };
        let actual = section(&doc, &names)?.and_then(|values| values.get(&edit.key));
        if actual == to {
            continue;
        }
        let desktop_sealed_key = edit.key == "inferenceGatewayApiKey"
            && actual
                .and_then(Value::as_str)
                .is_some_and(|key| key.starts_with("__sealed__:v1:"))
            && from.is_some_and(Value::is_string);
        if actual != from && !desktop_sealed_key {
            return Err("模型配置字段已被其他程序修改，请刷新后重新应用".into());
        }
        set(&mut doc, &names, &edit.key, to.cloned())?;
        if let Some(toml) = &mut toml {
            edit_toml(toml, &names, &edit.key, to)?;
        }
        if let Some(jsonc) = &mut jsonc {
            jsonc.set(&names, &edit.key, to)?;
        }
        changed = true;
    }
    if changed {
        let parent = delta.path.parent().ok_or("模型配置路径无效")?;
        let mut builder = fs::DirBuilder::new();
        builder.recursive(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(parent).map_err(|_| "无法创建模型配置目录")?;
        let contents = match (toml, jsonc) {
            (Some(toml), _) => toml.to_string().into_bytes(),
            (_, Some(jsonc)) => jsonc.into_bytes(),
            _ => json_bytes(&doc)?,
        };
        files::replace(&delta.path, bytes.as_deref(), &contents)?;
    }
    Ok(())
}

#[cfg(test)]
pub fn write_single(plan: Plan) -> Result<(), String> {
    if let Some(delta) = plan.finish() {
        apply(&delta, false)?;
    }
    Ok(())
}

fn pending_file(root: &Path) -> PathBuf {
    root.join("model-switch-pending.bin")
}

fn mcp_record(root: &Path, client: Client, slot: Option<&str>) -> PathBuf {
    match slot {
        Some(slot) => root.join(format!("previous-mcp-{}-{slot}.bin", client.id())),
        None => root.join(format!("previous-mcp-{}.bin", client.id())),
    }
}

pub fn commit_mcp(root: &Path, client: Client, slot: Option<&str>, plan: Plan) -> Result<(), String> {
    let Some(delta) = plan.finish() else {
        return Ok(());
    };
    let path = mcp_record(root, client, slot);
    let old = files::read(&path)?;
    vault::write(root, &path, &json_bytes(&delta)?)?;
    if let Err(error) = apply(&delta, false) {
        let current = files::read(&path)?;
        if let Some(old) = old {
            files::replace(&path, current.as_deref(), &old)?;
        } else {
            fs::remove_file(path).map_err(|_| "无法清理 MCP 变更备份")?;
        }
        return Err(error);
    }
    Ok(())
}

pub fn can_undo_mcp(root: &Path, client: Client, slot: Option<&str>, config: &Path) -> bool {
    vault::read(root, &mcp_record(root, client, slot))
        .ok()
        .flatten()
        .and_then(|bytes| serde_json::from_slice::<Delta>(&bytes).ok())
        .is_some_and(|delta| delta.at(config))
}

pub fn undo_mcp(root: &Path, client: Client, slot: Option<&str>, config: &Path) -> Result<(), String> {
    let path = mcp_record(root, client, slot);
    let bytes = vault::read(root, &path)?.ok_or("没有可撤销的 MCP 变更")?;
    let delta: Delta = serde_json::from_slice(&bytes).map_err(|_| "MCP 变更备份格式无效")?;
    if delta.path != config {
        return Err("MCP 配置目录已变化，无法撤销之前的变更".into());
    }
    apply(&delta, true)?;
    fs::remove_file(path).map_err(|_| "已恢复 MCP 配置，无法清理撤销记录".into())
}

fn read_journal(root: &Path, path: &Path) -> Result<Option<Journal>, String> {
    vault::read(root, path)?
        .map(|bytes| serde_json::from_slice(&bytes).map_err(|_| "本机模型切换记录无效".into()))
        .transpose()
}

pub fn recover(root: &Path, paths: &[PathBuf]) -> Result<(), String> {
    let pending = pending_file(root);
    let Some(journal) = read_journal(root, &pending)? else {
        return Ok(());
    };
    validate(&journal, paths)?;
    let committed = read_journal(root, &journal.client.backup(root))?
        .is_some_and(|saved| saved.id == journal.id);
    if !committed {
        for delta in journal.deltas.iter().rev() {
            apply(delta, true)?;
        }
    }
    fs::remove_file(pending).map_err(|_| "无法清理本机模型切换记录".into())
}

pub fn commit(
    root: &Path,
    client: Client,
    deltas: Vec<Delta>,
    paths: &[PathBuf],
) -> Result<(), String> {
    if deltas.is_empty() {
        return Ok(());
    }
    let journal = Journal {
        id: vault::id()?,
        client,
        deltas,
    };
    validate(&journal, paths)?;
    let record = json_bytes(&journal)?;
    let pending = pending_file(root);
    vault::write(root, &pending, &record)?;
    let mut applied = Vec::new();
    let result = (|| {
        for delta in &journal.deltas {
            apply(delta, false)?;
            applied.push(delta);
        }
        vault::write(root, &client.backup(root), &record)
    })();
    if let Err(error) = result {
        for delta in applied.into_iter().rev() {
            if apply(delta, true).is_err() {
                return Err("模型切换中断，部分字段等待恢复。请检查模型管理状态".into());
            }
        }
        let _ = fs::remove_file(pending);
        return Err(error);
    }
    let _ = fs::remove_file(pending);
    Ok(())
}

pub fn can_undo(root: &Path, client: Client) -> bool {
    client.backup(root).is_file()
}

pub fn prepare_undo(root: &Path, client: Client, paths: &[PathBuf]) -> Result<Vec<Delta>, String> {
    let mut journal = read_journal(root, &client.backup(root))?.ok_or("没有可以撤销的切换")?;
    validate(&journal, paths)?;
    journal.deltas.reverse();
    for delta in &mut journal.deltas {
        for edit in &mut delta.edits {
            std::mem::swap(&mut edit.before, &mut edit.after);
        }
    }
    Ok(journal.deltas)
}

pub(crate) fn restored_gateway_port(delta: &Delta, state: &Path, client: Client) -> Option<u16> {
    let key = match client {
        Client::Desktop => "gateway",
        Client::Codex => "codex_gateway",
        _ => return None,
    };
    if delta.path != state {
        return None;
    }
    delta.edits.iter().find_map(|edit| {
        if edit.section.is_some() || edit.key != client.id() {
            return None;
        }
        let selection = edit.after.value()?.get(key)?.as_object()?;
        Some(
            selection
                .get("port")
                .and_then(Value::as_u64)
                .and_then(|port| u16::try_from(port).ok())
                .filter(|port| *port != 0)
                .unwrap_or(crate::gateway::DEFAULT_PORT),
        )
    })
}

pub fn undo(root: &Path, client: Client, paths: &[PathBuf]) -> Result<(), String> {
    commit(root, client, prepare_undo(root, client, paths)?, paths)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn concurrent_field_change_rolls_back_prior_writes() {
        let dir = tempfile::tempdir().unwrap();
        let first = dir.path().join("a.json");
        let second = dir.path().join("b.json");
        fs::write(&first, br#"{"selected":"old","keep":true}"#).unwrap();
        fs::write(&second, br#"{"appliedId":"old"}"#).unwrap();
        let mut a = Plan::open(&first).unwrap();
        let mut b = Plan::open(&second).unwrap();
        a.set(None, "selected", Some(json!("new"))).unwrap();
        b.set(None, "appliedId", Some(Value::Null)).unwrap();
        fs::write(&second, br#"{"appliedId":"external"}"#).unwrap();
        assert!(commit(
            dir.path(),
            Client::Desktop,
            vec![a.finish().unwrap(), b.finish().unwrap()],
            &[first.clone(), second.clone()]
        )
        .is_err());
        assert_eq!(
            Plan::open(&first).unwrap().get(None, "selected").unwrap(),
            Some(&json!("old"))
        );
        assert_eq!(
            Plan::open(&second).unwrap().get(None, "appliedId").unwrap(),
            Some(&json!("external"))
        );
        assert!(!can_undo(dir.path(), Client::Desktop));
    }
}

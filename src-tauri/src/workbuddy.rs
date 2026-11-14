use crate::{
    clients::{ClientView, Outcome},
    model_transport as http,
    models::{self, Store},
    transaction::{self, Client, Plan},
};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

const VENDOR: &str = "codeneo-lite";
const LEGACY_VENDOR: &str = "CodeNeo Lite";

pub fn model_fields(id: &str, name: &str, url: &str, key: &str) -> Value {
    json!({"id":id, "name":name, "vendor":VENDOR, "url":url, "apiKey":key})
}

pub fn path() -> Result<PathBuf, String> {
    configured_file("models.json")
}

pub fn configured_file(name: &str) -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or("找不到 WorkBuddy 的配置目录")?;
    let directories = [home.join(".workbuddy"), home.join(".workbuddy-ai")];
    let mut existing = directories
        .iter()
        .map(|dir| dir.join(name))
        .filter(|path| path.is_file());
    if let Some(first) = existing.next() {
        if existing.next().is_some() {
            return Err("找到多个 WorkBuddy 配置文件，请在 WorkBuddy 设置中核对当前版本。".into());
        }
        return Ok(first);
    }
    let mut initialized = directories.iter().filter(|dir| dir.is_dir());
    if let Some(dir) = initialized.next() {
        if initialized.next().is_some() {
            return Err(
                "找到两个 WorkBuddy 用户目录。请先在当前版本保存一项配置，再回来刷新。".into(),
            );
        }
        return Ok(dir.join(name));
    }
    Err("请先打开 WorkBuddy，进入头像 → 设置 → 模型，再回来刷新。".into())
}

fn array<'a>(plan: &'a Plan, key: &str) -> Result<&'a [Value], String> {
    plan.get(None, key)?
        .map(|items| {
            items
                .as_array()
                .map(Vec::as_slice)
                .ok_or_else(|| format!("WorkBuddy 配置中的 {key} 必须为数组"))
        })
        .transpose()
        .map(|items| items.unwrap_or_default())
}

pub fn view(path: &Path) -> Result<ClientView, String> {
    let plan = Plan::open(path)?;
    let entries = array(&plan, "models")?;
    let visible = array(&plan, "availableModels")?;
    let models: Vec<_> = entries
        .iter()
        .filter(|entry| visible.is_empty() || visible.contains(&entry["id"]))
        .filter_map(|entry| entry["name"].as_str().or_else(|| entry["id"].as_str()))
        .map(str::to_owned)
        .collect();
    Ok(ClientView {
        state: "ready",
        account: format!("{} 个自定义模型", models.len()),
        model: "在 WorkBuddy 对话中选择".into(),
        models,
        ..ClientView::official()
    })
}

pub fn add(
    root: &Path,
    path: &Path,
    store: &Store,
    id: &str,
    paths: &[PathBuf],
) -> Result<Outcome, String> {
    add_many(root, path, store, &[id], paths)
}

pub fn add_many(
    root: &Path,
    path: &Path,
    store: &Store,
    ids: &[&str],
    paths: &[PathBuf],
) -> Result<Outcome, String> {
    let mut plan = Plan::open(path)?;
    let mut registered = array(&plan, "models")?.to_vec();
    let mut visible = array(&plan, "availableModels")?.to_vec();
    let limited = !visible.is_empty();
    for id in ids {
        let (model, account) = store.model(id)?;
        models::require_model(Client::Workbuddy, model, account)?;
        let model_id = http::provider_api_model(&model.model)?;
        let key = models::key(root, account)?;
        let fields = model_fields(
            &model_id,
            &model.name,
            &models::openai_endpoint(
                account,
                models::Protocol::ChatCompletions,
                "/chat/completions",
            ),
            &key,
        );
        if let Some(existing) = registered.iter_mut().find(|entry| entry["id"] == model_id) {
            if !matches!(existing["vendor"].as_str(), Some(VENDOR | LEGACY_VENDOR)) {
                return Err("WorkBuddy 已有同名模型，请在 WorkBuddy 中核对现有配置".into());
            }
            existing
                .as_object_mut()
                .expect("entry with model id is an object")
                .extend(
                    fields
                        .as_object()
                        .expect("model fields are an object")
                        .clone(),
                );
        } else {
            registered.push(fields);
        }
        if limited
            && !visible
                .iter()
                .any(|entry| entry.as_str() == Some(model_id.as_str()))
        {
            visible.push(json!(model_id));
        }
    }
    if limited {
        plan.set(None, "availableModels", Some(json!(visible)))?;
    }
    plan.set(None, "models", Some(json!(registered)))?;
    transaction::commit(
        root,
        Client::Workbuddy,
        plan.finish().into_iter().collect(),
        paths,
    )?;
    Ok(Outcome {
        message: format!(
            "已向 WorkBuddy 添加 {} 个模型。可在对话的模型菜单中切换。",
            ids.len()
        )
        .into(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn workbuddy_adds_multiple_models_and_undo_restores_its_picker() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let path = root.join("workbuddy/models.json");
        fs::create_dir(path.parent().unwrap()).unwrap();
        let previous = json!({"models": [{"id": "existing", "name": "已有模型"}], "availableModels": ["existing"], "keep": true});
        fs::write(&path, serde_json::to_vec(&previous).unwrap()).unwrap();
        let paths = [path.clone(), models::path(root)];
        for id in ["gpt-6.1-sol", "gpt-5.4-mini"] {
            let input = serde_json::from_value(json!({
                "id": null, "account_id": null, "preset": "openai", "account_name": "Fixture", "name": "",
                "base_url": "", "auth": "auth_token", "model": id, "key": "fixture-key"
            })).unwrap();
            let id = models::save(root, input, None).unwrap();
            let (_, store) = models::read(root).unwrap();
            add(root, &path, &store, &id, &paths).unwrap();
        }
        let current: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(current["models"][0], previous["models"][0]);
        assert_eq!(
            current["models"][1]["url"],
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(
            current["availableModels"],
            json!(["existing", "gpt-6.1-sol", "gpt-5.4-mini"])
        );
        transaction::undo(root, Client::Workbuddy, &paths).unwrap();
        let current: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            current["availableModels"],
            json!(["existing", "gpt-6.1-sol"])
        );
        assert_eq!(current["keep"], true);
    }

    #[test]
    fn adding_a_batch_to_workbuddy_is_one_undoable_change() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let path = root.join("workbuddy.json");
        let previous = json!({"models": [{"id": "native"}], "availableModels": ["native"]});
        fs::write(&path, serde_json::to_vec(&previous).unwrap()).unwrap();
        let paths = [path.clone(), models::path(root)];
        let ids = models::save_models(
            root,
            serde_json::from_value(json!({
                "preset": "deepseek", "account_name": "", "base_url": "", "auth": "auth_token",
                "name": "", "model": "deepseek-flash", "key": "fixture-key"
            }))
            .unwrap(),
            Some(Client::Workbuddy),
            serde_json::from_value(json!([{"model": "deepseek-v4-pro"}])).unwrap(),
        )
        .unwrap();
        let (_, store) = models::read(root).unwrap();
        let ids: Vec<_> = ids.iter().map(String::as_str).collect();
        add_many(root, &path, &store, &ids, &paths).unwrap();
        let current: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(
            current["availableModels"],
            json!(["native", "deepseek-flash", "deepseek-v4-pro"])
        );
        assert_eq!(
            current["models"][1]["url"],
            "https://api.deepseek.com/chat/completions"
        );
        assert_eq!(
            current["models"][1]["apiKey"],
            current["models"][2]["apiKey"]
        );
        transaction::undo(root, Client::Workbuddy, &paths).unwrap();
        assert_eq!(
            serde_json::from_slice::<Value>(&fs::read(&path).unwrap()).unwrap(),
            previous
        );
    }
}

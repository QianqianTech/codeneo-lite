use crate::files;
use ring::rand::{SecureRandom, SystemRandom};
use std::path::Path;

pub fn id() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| "无法生成本机配置编号")?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(unix)]
fn key(root: &Path, create: bool) -> Result<ring::aead::LessSafeKey, String> {
    use ring::aead::{LessSafeKey, UnboundKey, AES_256_GCM};
    let path = root.join("model-vault.key");
    let bytes = match files::read(&path)? {
        Some(bytes) => bytes,
        None if create => {
            let mut bytes = vec![0u8; 32];
            SystemRandom::new()
                .fill(&mut bytes)
                .map_err(|_| "无法生成本机加密密钥")?;
            files::replace(&path, None, &bytes)?;
            bytes
        }
        None => return Err("本机模型凭据的加密密钥已丢失".into()),
    };
    UnboundKey::new(&AES_256_GCM, &bytes)
        .map(LessSafeKey::new)
        .map_err(|_| "本机加密密钥无效".into())
}

#[cfg(unix)]
pub fn protect(root: &Path, data: &[u8]) -> Result<Vec<u8>, String> {
    use ring::aead::{Aad, Nonce};
    let mut nonce = [0u8; 12];
    SystemRandom::new()
        .fill(&mut nonce)
        .map_err(|_| "无法加密模型配置")?;
    let mut body = data.to_vec();
    key(root, true)?
        .seal_in_place_append_tag(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(b"ClaudeKeeper Lite model data v1"),
            &mut body,
        )
        .map_err(|_| "无法加密模型配置")?;
    let mut out = nonce.to_vec();
    out.extend(body);
    Ok(out)
}

#[cfg(unix)]
pub fn unprotect(root: &Path, data: &[u8]) -> Result<Vec<u8>, String> {
    use ring::aead::{Aad, Nonce};
    let (nonce, body) = data.split_at_checked(12).ok_or("本机模型配置备份无效")?;
    let mut body = body.to_vec();
    let clear = key(root, false)?
        .open_in_place(
            Nonce::try_assume_unique_for_key(nonce).map_err(|_| "备份无效")?,
            Aad::from(b"ClaudeKeeper Lite model data v1"),
            &mut body,
        )
        .map_err(|_| "本机模型配置无法解密")?;
    Ok(clear.to_vec())
}

#[cfg(windows)]
pub fn protect(_: &Path, data: &[u8]) -> Result<Vec<u8>, String> {
    crate::secrets::seal(data)
}
#[cfg(windows)]
pub fn unprotect(_: &Path, data: &[u8]) -> Result<Vec<u8>, String> {
    crate::secrets::unseal(data)
}

pub fn write(root: &Path, path: &Path, data: &[u8]) -> Result<(), String> {
    let previous = files::read(path)?;
    files::replace(path, previous.as_deref(), &protect(root, data)?)
}

pub fn read(root: &Path, path: &Path) -> Result<Option<Vec<u8>>, String> {
    files::read(path)?
        .map(|bytes| unprotect(root, &bytes))
        .transpose()
}

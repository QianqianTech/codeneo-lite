use crate::files;
use std::path::{Path, PathBuf};

pub fn root() -> Result<PathBuf, String> {
    #[cfg(debug_assertions)]
    if let Some(path) = std::env::var_os("CODENEO_LITE_DATA_DIR") {
        let path = PathBuf::from(path);
        return if path.is_absolute() {
            Ok(path)
        } else {
            Err("CODENEO_LITE_DATA_DIR 必须使用绝对路径".into())
        };
    }
    dirs::data_local_dir()
        .map(|dir| dir.join("app.claudekeeper.lite"))
        .ok_or_else(|| "找不到 Lite 的本机资料目录".into())
}

pub fn key_path(root: &Path, slot: &str) -> Result<PathBuf, String> {
    if slot.len() != 64 || !slot.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("FrameLeap 凭据引用无效，请在 Lite 重新配置".into());
    }
    Ok(root.join("frameleap").join(format!("{slot}.key")))
}

pub fn validated(key: &str) -> Result<&str, String> {
    let key = key.trim();
    if !key.strip_prefix("fl_").is_some_and(|value| {
        (32..=96).contains(&value.len())
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    }) {
        return Err("请输入有效的 FrameLeap 工作区 MCP Key（以 fl_ 开头）".into());
    }
    Ok(key)
}

pub fn load(root: &Path, slot: &str) -> Result<Option<String>, String> {
    let path = key_path(root, slot)?;
    let Some(bytes) = files::read(&path)? else {
        return Ok(None);
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = path
            .symlink_metadata()
            .map_err(|_| "无法检查 FrameLeap 凭据文件")?;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o077 != 0 {
            return Err("FrameLeap 凭据文件权限异常，请在 Lite 重新配置".into());
        }
    }
    let clear = unseal(&bytes)?;
    let key = String::from_utf8(clear).map_err(|_| "FrameLeap 凭据损坏，请在 Lite 重新配置")?;
    validated(&key)?;
    Ok(Some(key))
}

#[cfg(unix)]
pub fn seal(bytes: &[u8]) -> Result<Vec<u8>, String> {
    Ok(bytes.to_vec())
}

#[cfg(unix)]
fn unseal(bytes: &[u8]) -> Result<Vec<u8>, String> {
    Ok(bytes.to_vec())
}

#[cfg(windows)]
pub fn seal(bytes: &[u8]) -> Result<Vec<u8>, String> {
    dpapi(bytes, true)
}

#[cfg(windows)]
pub(crate) fn unseal(bytes: &[u8]) -> Result<Vec<u8>, String> {
    dpapi(bytes, false)
}

#[cfg(windows)]
fn dpapi(bytes: &[u8], protect: bool) -> Result<Vec<u8>, String> {
    use std::ptr;
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{
            CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
        },
    };
    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(bytes.len()).map_err(|_| "本机凭据过大")?,
        pbData: bytes.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: ptr::null_mut(),
    };
    let ok = unsafe {
        if protect {
            CryptProtectData(
                &input,
                ptr::null(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                ptr::null_mut(),
                ptr::null(),
                ptr::null(),
                ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    if ok == 0 {
        return Err("Windows 无法保护或读取当前账户的 FrameLeap 凭据".into());
    }
    let result = if output.cbData == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() }
    };
    unsafe { LocalFree(output.pbData.cast()) };
    Ok(result)
}

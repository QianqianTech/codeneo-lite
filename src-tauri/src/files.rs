use std::{
    fs::{self, File, TryLockError},
    io::{ErrorKind, Read, Write},
    path::Path,
};

pub const LIMIT: usize = 1024 * 1024;

pub(crate) fn linked(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return true;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.is_file() && metadata.nlink() != 1 {
            return true;
        }
    }
    metadata.is_symlink()
}

pub fn read(path: &Path) -> Result<Option<Vec<u8>>, String> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err("无法读取本机配置的文件信息".into()),
    };
    if linked(&metadata) || !metadata.is_file() || metadata.len() > LIMIT as u64 {
        return Err("本机配置不是普通文件或超过大小限制，未修改".into());
    }
    let mut options = fs::OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path).map_err(|_| "无法读取本机配置")?;
    if !file.metadata().map_err(|_| "无法检查本机配置")?.is_file() {
        return Err("本机配置不是普通文件，未修改".into());
    }
    let mut bytes = Vec::new();
    file.take(LIMIT as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "无法读取本机配置")?;
    if bytes.len() > LIMIT {
        return Err("本机配置超过大小限制，未修改".into());
    }
    Ok(Some(bytes))
}

pub fn private_dir(path: &Path) -> Result<(), String> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path).map_err(|_| "无法创建 Lite 私有目录")?;
    let metadata = fs::symlink_metadata(path).map_err(|_| "无法检查 Lite 私有目录")?;
    if !metadata.is_dir() || linked(&metadata) {
        return Err("Lite 私有目录不是普通目录".into());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.uid() != unsafe { libc::geteuid() } {
            return Err("Lite 私有目录不属于当前用户".into());
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))
            .map_err(|_| "无法保护 Lite 私有目录")?;
    }
    Ok(())
}

// Errors are returned before the rename. After it succeeds, callers can safely
// treat the file as committed without rolling back a referenced credential.
pub fn replace(path: &Path, expected: Option<&[u8]>, contents: &[u8]) -> Result<(), String> {
    if contents.len() > LIMIT {
        return Err("更新后的配置超过大小限制".into());
    }
    let parent = path.parent().ok_or("无效的本机配置路径")?;
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|_| "无法暂存本机配置")?;
    temp.write_all(contents)
        .and_then(|_| temp.as_file().sync_all())
        .map_err(|_| "无法写入本机配置")?;
    if read(path)?.as_deref() != expected {
        return Err("配置已被其他程序修改，请重试".into());
    }
    temp.persist(path).map_err(|_| "无法提交本机配置")?;
    #[cfg(unix)]
    let _ = File::open(parent).and_then(|dir| dir.sync_all());
    Ok(())
}

pub fn lock(root: &Path) -> Result<File, String> {
    private_dir(root)?;
    try_lock(&root.join("configure.lock"))?
        .ok_or_else(|| "已有 Lite 窗口正在配置，请稍后再试".into())
}

pub(crate) fn try_lock(path: &Path) -> Result<Option<File>, String> {
    read(path)?;
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let lock = options.open(path).map_err(|_| "无法打开 Lite 锁文件")?;
    match lock.try_lock() {
        Ok(()) => Ok(Some(lock)),
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(error)) => Err(format!("无法锁定 Lite 配置（{error}）。")),
    }
}

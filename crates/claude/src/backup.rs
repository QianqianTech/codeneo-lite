use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum BackupError {
    Io(std::io::Error),
    Outside,
    Missing,
}

impl std::fmt::Display for BackupError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BackupError::Io(err) => write!(f, "{err}"),
            BackupError::Outside => write!(f, "还原路径必须在备份目录里"),
            BackupError::Missing => write!(f, "没有可备份的 Claude 目录"),
        }
    }
}

pub fn backup_claude_cleanup(home: &Path, stamp: &str) -> Result<PathBuf, BackupError> {
    if !valid_stamp(stamp) {
        return Err(BackupError::Io(std::io::Error::new(std::io::ErrorKind::InvalidInput, "备份时间无效")));
    }
    let root = home.join("ClaudeBackups");
    fs::create_dir_all(&root).map_err(BackupError::Io)?;
    if fs::symlink_metadata(&root).map_err(BackupError::Io)?.file_type().is_symlink() {
        return Err(BackupError::Outside);
    }
    let mut suffix = 0;
    let dest = loop {
        let name = if suffix == 0 { format!("claude-cleanup-{stamp}") } else { format!("claude-cleanup-{stamp}-{suffix}") };
        let path = root.join(name);
        match fs::create_dir(&path) {
            Ok(()) => break path,
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => suffix += 1,
            Err(err) => return Err(BackupError::Io(err)),
        }
    };
    restrict_owner(&root)?;
    restrict_owner(&dest)?;
    let copied = (|| -> Result<(), BackupError> {
        let mut count = 0;
        let claude = home.join(".claude");
        if claude.is_dir() { copy_any(&claude, &dest.join(".claude"))?; count += 1; }
        let config = home.join(".claude.json");
        if config.is_file() { copy_any(&config, &dest.join(".claude.json"))?; count += 1; }
        if count == 0 { return Err(BackupError::Missing); }
        Ok(())
    })();
    if let Err(error) = copied {
        let _ = fs::remove_dir_all(&dest);
        return Err(error);
    }
    Ok(dest)
}

fn valid_stamp(stamp: &str) -> bool {
    !stamp.is_empty() && stamp.len() <= 32 && stamp.chars().any(|c| c.is_ascii_digit()) && stamp.chars().all(|c| c.is_ascii_digit() || c == '-')
}

#[cfg(unix)]
fn restrict_owner(path: &Path) -> Result<(), BackupError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).map_err(BackupError::Io)
}

#[cfg(not(unix))]
fn restrict_owner(_path: &Path) -> Result<(), BackupError> {
    Ok(())
}

pub fn backup_tree(sources: &[PathBuf], dest_root: &Path) -> Result<PathBuf, BackupError> {
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
    let dest = dest_root.join(format!("claude-{stamp}"));
    fs::create_dir_all(&dest).map_err(BackupError::Io)?;
    let mut copied = 0;
    for source in sources {
        if !source.exists() {
            continue;
        }
        let name = source.file_name().unwrap_or_default();
        copy_any(source, &dest.join(name))?;
        copied += 1;
    }
    if copied == 0 {
        return Err(BackupError::Missing);
    }
    Ok(dest)
}

pub fn restore_tree(backup: &Path, dest_root: &Path, targets: &[(PathBuf, PathBuf)]) -> Result<(), BackupError> {
    let backup = backup.canonicalize().map_err(BackupError::Io)?;
    let root = dest_root.canonicalize().map_err(BackupError::Io)?;
    if !backup.starts_with(&root) {
        return Err(BackupError::Outside);
    }
    for (name, target) in targets {
        let source = backup.join(name);
        if source.exists() {
            if target.exists() {
                if target.is_dir() {
                    fs::remove_dir_all(target).map_err(BackupError::Io)?;
                } else {
                    fs::remove_file(target).map_err(BackupError::Io)?;
                }
            }
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).map_err(BackupError::Io)?;
            }
            copy_any(&source, target)?;
        }
    }
    Ok(())
}

fn copy_any(from: &Path, to: &Path) -> Result<(), BackupError> {
    let meta = fs::symlink_metadata(from).map_err(BackupError::Io)?;
    if meta.file_type().is_symlink() || !(meta.is_file() || meta.is_dir()) {
        return Err(BackupError::Outside);
    }
    if from.is_dir() {
        copy_dir(from, to)
    } else {
        if let Some(parent) = to.parent() {
            fs::create_dir_all(parent).map_err(BackupError::Io)?;
        }
        fs::copy(from, to).map_err(BackupError::Io)?;
        #[cfg(unix)] {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(to, fs::Permissions::from_mode(0o600)).map_err(BackupError::Io)?;
        }
        Ok(())
    }
}

fn copy_dir(from: &Path, to: &Path) -> Result<(), BackupError> {
    fs::create_dir_all(to).map_err(BackupError::Io)?;
    restrict_owner(to)?;
    for entry in fs::read_dir(from).map_err(BackupError::Io)? {
        let entry = entry.map_err(BackupError::Io)?;
        let target = to.join(entry.file_name());
        copy_any(&entry.path(), &target)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_puts_machine_id_back_and_ignores_routes() {
        let root = std::env::temp_dir().join(format!("ck-backup-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let home = root.join("home");
        let claude = home.join(".claude");
        fs::create_dir_all(claude.join("projects")).unwrap();
        fs::write(home.join(".claude.json"), r#"{"machineID":"keep-me"}"#).unwrap();
        fs::write(claude.join("settings.json"), "{}").unwrap();
        let routes = root.join("routes.txt");
        fs::write(&routes, "original-route").unwrap();
        let backups = root.join("backups");
        let saved = backup_tree(&[home.join(".claude"), home.join(".claude.json")], &backups).unwrap();
        fs::write(home.join(".claude.json"), r#"{"machineID":"changed"}"#).unwrap();
        restore_tree(
            &saved,
            &backups,
            &[
                (PathBuf::from(".claude"), home.join(".claude")),
                (PathBuf::from(".claude.json"), home.join(".claude.json")),
            ],
        )
        .unwrap();
        assert_eq!(fs::read_to_string(home.join(".claude.json")).unwrap(), r#"{"machineID":"keep-me"}"#);
        assert_eq!(fs::read_to_string(&routes).unwrap(), "original-route");
        let outside = root.join("other");
        fs::create_dir_all(&outside).unwrap();
        assert!(restore_tree(&outside, &backups, &[]).is_err());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cleanup_backup_copies_claude_home_files() {
        let root = std::env::temp_dir().join(format!("ck-cleanup-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let home = root.join("home");
        fs::create_dir_all(home.join(".claude")).unwrap();
        fs::write(home.join(".claude").join("settings.json"), "{}").unwrap();
        fs::write(home.join(".claude.json"), r#"{"machineID":"keep-me"}"#).unwrap();
        let saved = backup_claude_cleanup(&home, "20261027-005500").unwrap();
        assert_eq!(saved.file_name().unwrap(), "claude-cleanup-20261027-005500");
        assert_eq!(saved.parent().unwrap().file_name().unwrap(), "ClaudeBackups");
        assert_eq!(fs::read_to_string(saved.join(".claude.json")).unwrap(), r#"{"machineID":"keep-me"}"#);
        assert_eq!(fs::read_to_string(saved.join(".claude").join("settings.json")).unwrap(), "{}");
        assert_eq!(fs::read_to_string(home.join(".claude.json")).unwrap(), r#"{"machineID":"keep-me"}"#);
        let repeated = backup_claude_cleanup(&home, "20261027-005500").unwrap();
        assert_ne!(saved, repeated);
        assert!(saved.is_dir() && repeated.is_dir());
        assert!(backup_claude_cleanup(&home, "../evil").is_err());
        let empty = root.join("empty");
        fs::create_dir_all(&empty).unwrap();
        assert!(matches!(backup_claude_cleanup(&empty, "20261027-005501"), Err(BackupError::Missing)));
        assert!(!empty.join("ClaudeBackups").join("claude-cleanup-20261027-005501").exists());
        let _ = fs::remove_dir_all(root);
    }
}

// Explicit opt-in only. Never follow symlinks or permit nested repository metadata.
// Git may collapse an ignored parent directory, so inspect all descendants rather
// than granting permission to the entire parent from one allowed child.
use std::fs;
use std::path::{Component, Path, PathBuf};

pub(super) fn allowed(root: &Path, relative: &Path, allow: &[PathBuf]) -> Result<bool, String> {
    if allow.is_empty()
        || relative.as_os_str().is_empty()
        || !relative
            .components()
            .all(|c| matches!(c, Component::Normal(name) if !name.to_string_lossy().eq_ignore_ascii_case(".git")))
    {
        return Ok(false);
    }
    let mut parent = root.to_path_buf();
    for part in relative.components() {
        parent.push(part);
        let metadata = fs::symlink_metadata(&parent).map_err(|e| e.to_string())?;
        if metadata.file_type().is_symlink() {
            // Symlink removal is not recursive, but fail closed for path aliases.
            return Ok(false);
        }
    }
    let mut pending = vec![relative.to_path_buf()];
    while let Some(path) = pending.pop() {
        if path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case(".git"))
        {
            return Ok(false);
        }
        let metadata = fs::symlink_metadata(root.join(&path)).map_err(|e| e.to_string())?;
        let permitted = allow.iter().any(|entry| path.starts_with(entry));
        if metadata.is_dir() {
            // Bare repositories have no .git entry.
            if root.join(&path).join("HEAD").exists()
                && root.join(&path).join("objects").exists()
                && root.join(&path).join("refs").exists()
            {
                return Ok(false);
            }
            let mut children = 0;
            if !permitted && !allow.iter().any(|entry| entry.starts_with(&path)) {
                return Ok(false);
            }
            for entry in fs::read_dir(root.join(&path)).map_err(|e| e.to_string())? {
                children += 1;
                pending.push(path.join(entry.map_err(|e| e.to_string())?.file_name()));
            }
            if !permitted && children == 0 {
                return Ok(false);
            }
        } else if !permitted || (!metadata.is_file() && !metadata.file_type().is_symlink()) {
            return Ok(false);
        }
    }
    Ok(true)
}

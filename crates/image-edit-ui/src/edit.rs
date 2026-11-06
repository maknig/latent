//! File operations of the image editor: the backup of the untouched original and the atomic
//! overwrite.
//!
//! The backup layout is the one of latent (`images.rs`: `original_path`, `backup_original`,
//! `restore_original`): `<root>/.originals/<path relative to root>`, one backup per file made
//! only if none exists, restoring puts it back and removes it. Both programs can therefore restore
//! each other's backups. Share the code when this is merged into latent.

use std::path::{Path, PathBuf};

/// Folder of the backups inside an archive or save folder.
const ORIGINALS: &str = ".originals";

/// Where the backup of `file` (inside `root`) lives.
pub fn original_path(root: &Path, file: &Path) -> Result<PathBuf, String> {
    let rel = file
        .strip_prefix(root)
        .map_err(|_| format!("{} is outside {}", file.display(), root.display()))?;
    Ok(root.join(ORIGINALS).join(rel))
}

pub fn has_original(root: &Path, file: &Path) -> bool {
    original_path(root, file).is_ok_and(|p| p.is_file())
}

/// Copies `file` to its backup place. Does nothing when a backup exists already, so the first
/// original is the one that is kept however often it is called.
pub fn backup_original(root: &Path, file: &Path) -> Result<(), String> {
    let backup = original_path(root, file)?;
    if backup.is_file() {
        return Ok(());
    }
    let err = |e: std::io::Error| format!("backup of {}: {e}", file.display());
    if let Some(dir) = backup.parent() {
        std::fs::create_dir_all(dir).map_err(err)?;
    }
    std::fs::copy(file, &backup).map(|_| ()).map_err(err)
}

/// Puts the backup back over `file` and removes the backup.
pub fn restore_original(root: &Path, file: &Path) -> Result<(), String> {
    let backup = original_path(root, file)?;
    let err = |e: std::io::Error| format!("restore {}: {e}", file.display());
    let tmp = file.with_extension("restore.tmp");
    std::fs::copy(&backup, &tmp).map_err(err)?;
    std::fs::rename(&tmp, file).map_err(err)?;
    let _ = std::fs::remove_file(&backup);
    Ok(())
}

/// Runs `write` on a hidden temporary file next to `path` (same extension, so the format is
/// kept) and renames it over `path` when it succeeded; a failed write leaves `path` untouched.
pub fn overwrite_atomic(
    path: &Path,
    write: impl FnOnce(&Path) -> Result<(), String>,
) -> Result<(), String> {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned());
    let ext = path.extension().map(|s| s.to_string_lossy().into_owned());
    let name = format!(
        ".{}.edit.{}",
        stem.unwrap_or_default(),
        ext.unwrap_or_default()
    );
    let tmp = path.with_file_name(name);
    let result = write(&tmp).and_then(|()| {
        std::fs::rename(&tmp, path).map_err(|e| format!("replace {}: {e}", path.display()))
    });
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup(name: &str) -> (PathBuf, PathBuf) {
        let root = std::env::temp_dir().join(format!("scan-edit-{name}-{}", std::process::id()));
        let file = root.join("images/roll_film/0001/x.tiff");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(&file, b"original").unwrap();
        (root, file)
    }

    #[test]
    fn the_backup_uses_latents_layout() {
        let (root, file) = setup("layout");
        assert_eq!(
            original_path(&root, &file).unwrap(),
            root.join(".originals/images/roll_film/0001/x.tiff")
        );
        assert!(original_path(&root, Path::new("/elsewhere/x.tiff")).is_err());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_backup_is_made_once_and_the_first_original_is_kept() {
        let (root, file) = setup("once");
        assert!(!has_original(&root, &file));
        backup_original(&root, &file).unwrap();
        assert!(has_original(&root, &file));

        std::fs::write(&file, b"edited").unwrap();
        backup_original(&root, &file).unwrap(); // second call must not copy the edit
        let kept = std::fs::read(original_path(&root, &file).unwrap()).unwrap();
        assert_eq!(kept, b"original");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn restoring_puts_the_original_back_and_removes_the_backup() {
        let (root, file) = setup("restore");
        backup_original(&root, &file).unwrap();
        std::fs::write(&file, b"edited").unwrap();
        restore_original(&root, &file).unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"original");
        assert!(!has_original(&root, &file));
        assert!(!file.with_extension("restore.tmp").exists());
        // Nothing to restore any more.
        assert!(restore_original(&root, &file).is_err());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn overwriting_replaces_the_content_and_leaves_no_temp_file() {
        let (root, file) = setup("overwrite");
        overwrite_atomic(&file, |tmp| {
            assert_eq!(tmp.extension().unwrap(), "tiff", "the format is kept");
            std::fs::write(tmp, b"new").map_err(|e| e.to_string())
        })
        .unwrap();
        assert_eq!(std::fs::read(&file).unwrap(), b"new");
        let names: Vec<_> = std::fs::read_dir(file.parent().unwrap())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["x.tiff"], "no temp file left");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn a_failed_write_keeps_the_original_file() {
        let (root, file) = setup("fail");
        let result = overwrite_atomic(&file, |tmp| {
            std::fs::write(tmp, b"partial").unwrap();
            Err("disk full".into())
        });
        assert_eq!(result, Err("disk full".to_string()));
        assert_eq!(std::fs::read(&file).unwrap(), b"original");
        assert!(
            !file.with_file_name(".x.edit.tiff").exists(),
            "temp cleaned up"
        );
        std::fs::remove_dir_all(root).ok();
    }
}

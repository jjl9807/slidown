use crate::render::Deck;
use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fmt::Write,
    fs,
    path::{Component, Path, PathBuf},
};

pub fn digest(bytes: &[u8]) -> String {
    let hash = Sha256::digest(bytes);
    let mut hex = String::with_capacity(hash.len() * 2);
    for byte in hash {
        write!(hex, "{byte:02x}").expect("writing to a String cannot fail");
    }
    hex
}

/// An output directory owned entirely by the generator.
pub struct OutputDirectory {
    root: PathBuf,
}

impl OutputDirectory {
    fn new(output: &Path, dependencies: &BTreeSet<PathBuf>) -> Result<Self> {
        let absolute = std::path::absolute(output)?;
        let name = absolute
            .file_name()
            .context("output must be a dedicated directory")?;
        let parent = absolute
            .parent()
            .context("output must have a parent directory")?;
        fs::create_dir_all(parent)?;
        let root = fs::canonicalize(parent)?.join(name);
        ensure!(
            !fs::canonicalize(std::env::current_dir()?)?.starts_with(&root),
            "output must not be the working directory or one of its ancestors"
        );
        for input in dependencies {
            let resolved = fs::canonicalize(input)?;
            // Check the input's directory too, in case the input itself is a symlink.
            let entry = fs::canonicalize(input.parent().context("input has no parent")?)?
                .join(input.file_name().context("input has no filename")?);
            ensure!(
                !resolved.starts_with(&root) && !entry.starts_with(&root),
                "output directory must not contain input files: {}",
                input.display()
            );
        }
        let output = Self { root };
        output.check_root()?;
        Ok(output)
    }

    fn check_root(&self) -> Result<()> {
        match fs::symlink_metadata(&self.root) {
            Ok(metadata) => ensure!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "output must be a directory, not a file or symlink: {}",
                self.root.display()
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
            Err(error) => return Err(error.into()),
        }
        Ok(())
    }

    pub fn cleanup(self) -> Result<()> {
        self.check_root()?;
        match fs::remove_dir_all(&self.root) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => {
                Err(error).with_context(|| format!("remove output {}", self.root.display()))
            }
        }
    }
}

pub fn publish(
    deck: &Deck,
    output: &Path,
    dependencies: &BTreeSet<PathBuf>,
) -> Result<OutputDirectory> {
    let output = OutputDirectory::new(output, dependencies)?;
    let parent = output.root.parent().expect("validated output parent");
    let staged = tempfile::tempdir_in(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(staged.path(), fs::Permissions::from_mode(0o755))?;
    }
    // Prepare every file before touching the previous output. A fresh tree needs no manifest.
    for (name, bytes) in &deck.files {
        let relative = Path::new(name);
        ensure!(
            !name.is_empty()
                && relative
                    .components()
                    .all(|c| matches!(c, Component::Normal(_))),
            "invalid generated path: {name}"
        );
        let target = staged.path().join(relative);
        fs::create_dir_all(target.parent().expect("generated file parent"))?;
        fs::write(&target, bytes).with_context(|| format!("write generated file {name}"))?;
    }
    let backup = tempfile::tempdir_in(parent)?;
    let previous = backup.path().join("previous");
    output.check_root()?;
    if output.root.exists() {
        fs::rename(&output.root, &previous).context("move previous output aside")?;
    }
    if let Err(error) = fs::rename(staged.path(), &output.root) {
        if previous.exists()
            && let Err(restore) = fs::rename(&previous, &output.root)
        {
            let saved = backup.keep();
            bail!(
                "publish failed: {error}; restore failed: {restore}; previous output saved in {}",
                saved.display()
            );
        }
        return Err(error).context("publish output directory");
    }
    // Remove the entire previous tree, including stale images and manually added files.
    backup.close().context("remove previous output directory")?;
    Ok(output)
}

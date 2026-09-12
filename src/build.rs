use crate::render::Deck;
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Write,
    path::{Component, Path, PathBuf},
};

const MANIFEST: &str = "assets/.slidown-manifest.json";

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    generator: String,
    files: BTreeMap<String, String>,
}

pub fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn safe_target(root: &Path, relative: &str) -> Result<PathBuf> {
    let path = Path::new(relative);
    ensure!(
        !relative.is_empty() && path.components().all(|c| matches!(c, Component::Normal(_))),
        "invalid generated path: {relative}"
    );
    let mut target = root.to_owned();
    for component in path.components() {
        target.push(component);
        match fs::symlink_metadata(&target) {
            Ok(metadata) => ensure!(
                !metadata.file_type().is_symlink(),
                "refusing to write through symlink: {}",
                target.display()
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(e).with_context(|| format!("inspect {}", target.display())),
        }
    }
    Ok(target)
}

pub fn publish(deck: &Deck, output: &Path, dependencies: &BTreeSet<PathBuf>) -> Result<Artifacts> {
    fs::create_dir_all(output)
        .with_context(|| format!("create output directory {}", output.display()))?;
    let root = fs::canonicalize(output)?;
    let manifest_path = safe_target(&root, MANIFEST)?;
    let previous = if manifest_path.exists() {
        let manifest: Manifest = serde_json::from_slice(&fs::read(&manifest_path)?)
            .with_context(|| format!("invalid manifest {}", manifest_path.display()))?;
        ensure!(
            manifest.generator == "slidown-v1",
            "unrecognized output manifest"
        );
        manifest.files
    } else {
        BTreeMap::new()
    };
    let protected: BTreeSet<_> = dependencies
        .iter()
        .filter_map(|p| fs::canonicalize(p).ok())
        .collect();
    ensure!(
        !protected.contains(&manifest_path),
        "output manifest would overwrite an input file"
    );
    // Check every destination before changing any published content.
    let mut changed = Vec::new();
    for (name, bytes) in &deck.files {
        let target = safe_target(&root, name)?;
        if target.exists() {
            let old = fs::read(&target)
                .with_context(|| format!("read existing output {}", target.display()))?;
            if old == *bytes {
                continue;
            }
            ensure!(
                !protected.contains(&target),
                "output would overwrite input {}",
                target.display()
            );
            ensure!(
                previous.get(name) == Some(&digest(&old)),
                "output conflict: {} is not an unchanged slidown artifact; choose another --output directory",
                target.display()
            );
        }
        changed.push((target, bytes));
    }
    // Stage all content first; replace index.html last, after its assets are ready.
    changed.sort_by_key(|(p, _)| p == &root.join("index.html"));
    let mut staged = Vec::new();
    for (target, bytes) in changed {
        fs::create_dir_all(target.parent().expect("output parent"))?;
        let mut file = tempfile::NamedTempFile::new_in(target.parent().unwrap())?;
        file.write_all(bytes)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.as_file()
                .set_permissions(fs::Permissions::from_mode(0o644))?;
        }
        file.as_file().sync_all()?;
        staged.push((file, target));
    }
    for (file, target) in staged {
        file.persist(&target)
            .with_context(|| format!("publish {}", target.display()))?;
    }
    let manifest = Manifest {
        generator: "slidown-v1".into(),
        files: deck
            .files
            .iter()
            .map(|(p, bytes)| (p.clone(), digest(bytes)))
            .collect(),
    };
    fs::create_dir_all(manifest_path.parent().unwrap())?;
    let mut file = tempfile::NamedTempFile::new_in(manifest_path.parent().unwrap())?;
    let manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
    file.write_all(&manifest_bytes)?;
    file.persist(&manifest_path)?;
    // Remove only stale, untouched artifacts. Preserve user edits and all input files.
    for (name, expected) in previous {
        if deck.files.contains_key(&name) {
            continue;
        }
        if let Ok(path) = safe_target(&root, &name)
            && !protected.contains(&path)
            && fs::read(&path).is_ok_and(|bytes| digest(&bytes) == expected)
        {
            fs::remove_file(path)?;
        }
    }
    let mut files = manifest.files;
    files.insert(MANIFEST.into(), digest(&manifest_bytes));
    Ok(Artifacts { root, files })
}

/// Records exactly what this process published, independent of later manifest edits.
pub struct Artifacts {
    root: PathBuf,
    files: BTreeMap<String, String>,
}

impl Artifacts {
    pub fn cleanup(self, dependencies: &BTreeSet<PathBuf>) -> Result<()> {
        match fs::symlink_metadata(&self.root) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            metadata => {
                ensure!(
                    !metadata?.file_type().is_symlink()
                        && fs::canonicalize(&self.root)? == self.root,
                    "output directory changed since publication; refusing cleanup: {}",
                    self.root.display()
                );
            }
        }
        let protected: BTreeSet<_> = dependencies
            .iter()
            .filter_map(|path| fs::canonicalize(path).ok())
            .collect();
        let mut directories = BTreeSet::new();
        let mut errors = Vec::new();
        for (name, expected) in self.files {
            let target = match safe_target(&self.root, &name) {
                Ok(target) => target,
                Err(error) => {
                    eprintln!("warning: preserving {name} during cleanup: {error:#}");
                    continue;
                }
            };
            let mut parent = target.parent();
            while let Some(directory) = parent {
                if directory == self.root {
                    break;
                }
                directories.insert(directory.to_owned());
                parent = directory.parent();
            }
            let operation = (|| -> Result<()> {
                let bytes = match fs::read(&target) {
                    Ok(bytes) => bytes,
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
                    Err(error) => return Err(error.into()),
                };
                if protected.contains(&target) || digest(&bytes) != expected {
                    eprintln!(
                        "warning: preserving modified or source file {}",
                        target.display()
                    );
                    return Ok(());
                }
                fs::remove_file(&target)?;
                Ok(())
            })();
            if let Err(error) = operation {
                errors.push(format!("{}: {error:#}", target.display()));
            }
        }
        // Never remove the working directory, even for an explicit --output .
        if fs::canonicalize(std::env::current_dir()?)? != self.root {
            directories.insert(self.root);
        }
        let mut directories: Vec<_> = directories.into_iter().collect();
        directories.sort_by_key(|path| std::cmp::Reverse(path.components().count()));
        for directory in directories {
            match fs::remove_dir(&directory) {
                Ok(()) => (),
                Err(error)
                    if matches!(
                        error.kind(),
                        std::io::ErrorKind::NotFound | std::io::ErrorKind::DirectoryNotEmpty
                    ) => {}
                Err(error) => errors.push(format!("{}: {error}", directory.display())),
            }
        }
        ensure!(
            errors.is_empty(),
            "failed to clean preview artifacts:\n{}",
            errors.join("\n")
        );
        Ok(())
    }
}

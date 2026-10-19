//! Installing and removing plugins while PennyChest runs.
//!
//! A plugin is downloaded as a `.tar.gz` of its directory, checked against the checksum its
//! repository lists, unpacked beside the installed plugins and started once to check it works.
//! Only then does it replace the installed version, so a broken download or a plugin that
//! won't start leaves what was installed as it was.

use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use pennychest_plugin::manifest;
use sha2::{Digest, Sha256};

use super::{InstalledInfo, Plugin, Plugins};

/// The most an unpacked plugin may take on disk.
const MAX_UNPACKED: u64 = 500 * 1024 * 1024;
const MAX_FILES: usize = 10_000;

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error("the download doesn't match the checksum its repository lists")]
    Checksum,
    #[error("the download isn't a plugin PennyChest can install: {0}")]
    Archive(String),
    #[error("it doesn't work: {0}")]
    Broken(String),
    #[error("it isn't installed")]
    NotInstalled,
    #[error("the plugin directory couldn't be changed: {0}")]
    Io(#[from] std::io::Error),
}

/// Whether `archive` is what the repository promised.
pub fn checksum_matches(archive: &[u8], sha256: &str) -> bool {
    hex::encode(Sha256::digest(archive)).eq_ignore_ascii_case(sha256)
}

/// Unpacks a `.tar.gz` into `into`, allowing only plain files and directories that stay
/// inside it.
fn unpack(archive: &[u8], into: &Path) -> Result<(), InstallError> {
    let bad = |e: std::io::Error| InstallError::Archive(e.to_string());
    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(archive));
    let (mut total, mut files) = (0u64, 0usize);
    for entry in tar.entries().map_err(bad)? {
        let mut entry = entry.map_err(bad)?;
        let path = entry.path().map_err(bad)?.into_owned();
        let safe = path
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir));
        if !safe {
            return Err(InstallError::Archive(format!(
                "{} is outside the plugin",
                path.display()
            )));
        }
        let kind = entry.header().entry_type();
        if !(kind.is_file() || kind.is_dir()) {
            return Err(InstallError::Archive(format!(
                "{} isn't a plain file or directory",
                path.display()
            )));
        }
        files += 1;
        total += entry.header().size().map_err(bad)?;
        if files > MAX_FILES || total > MAX_UNPACKED {
            return Err(InstallError::Archive(format!(
                "it unpacks to more than {} MiB or {MAX_FILES} files",
                MAX_UNPACKED / 1024 / 1024
            )));
        }
        let target = into.join(&path);
        if kind.is_dir() {
            fs::create_dir_all(&target)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut content = Vec::new();
        (&mut entry)
            .take(MAX_UNPACKED)
            .read_to_end(&mut content)
            .map_err(bad)?;
        fs::write(&target, content)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // Executable bits only, for native plugins: nothing setuid or world-writable.
            let mode = entry.header().mode().map_err(bad)? & 0o755;
            fs::set_permissions(&target, fs::Permissions::from_mode(mode | 0o600))?;
        }
    }
    Ok(())
}

/// The plugin's own directory in what was unpacked: the top, or the one directory there.
fn plugin_root(unpacked: &Path) -> Result<PathBuf, InstallError> {
    if unpacked.join(manifest::FILE_NAME).is_file() {
        return Ok(unpacked.to_path_buf());
    }
    let entries: Vec<PathBuf> = fs::read_dir(unpacked)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .collect();
    match &entries[..] {
        [only] if only.join(manifest::FILE_NAME).is_file() => Ok(only.clone()),
        _ => Err(InstallError::Archive(format!(
            "there's no {} at its top",
            manifest::FILE_NAME
        ))),
    }
}

impl Plugins {
    /// Installs or updates the plugin `name` from a downloaded archive.
    pub fn install(
        &self,
        name: &str,
        archive: &[u8],
        sha256: &str,
    ) -> Result<InstalledInfo, InstallError> {
        if !checksum_matches(archive, sha256) {
            return Err(InstallError::Checksum);
        }
        let _changing = self.changing.lock().expect("not poisoned");
        fs::create_dir_all(&self.dir)?;
        let staging = tempfile::Builder::new()
            .prefix(".installing-")
            .tempdir_in(&self.dir)?;
        unpack(archive, staging.path())?;
        let root = plugin_root(staging.path())?;
        let plugin = Plugin::load(&root, self.timeouts).map_err(InstallError::Broken)?;
        if plugin.name() != name {
            return Err(InstallError::Archive(format!(
                "it's the {} plugin, not {name}",
                plugin.name()
            )));
        }
        let info = InstalledInfo {
            name: plugin.manifest.name.clone(),
            version: plugin.manifest.version.clone(),
            description: plugin.manifest.description.clone(),
        };
        drop(plugin);

        let installed = self.dir.join(name);
        let previous = staging.path().join(".previous");
        if installed.exists() {
            fs::rename(&installed, &previous)?;
        }
        if let Err(error) = fs::rename(&root, &installed) {
            if previous.exists() {
                fs::rename(&previous, &installed)?;
            }
            return Err(error.into());
        }
        // The staging directory, with the previous version in it, goes when it's dropped.
        drop(staging);
        self.reload();
        Ok(info)
    }

    /// Removes an installed plugin.
    pub fn uninstall(&self, name: &str) -> Result<(), InstallError> {
        if !manifest::valid_name(name) {
            return Err(InstallError::NotInstalled);
        }
        let _changing = self.changing.lock().expect("not poisoned");
        let installed = self.dir.join(name);
        if !installed.join(manifest::FILE_NAME).is_file() {
            return Err(InstallError::NotInstalled);
        }
        fs::remove_dir_all(&installed)?;
        self.reload();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn archive(files: &[(&str, &[u8], u32)]) -> Vec<u8> {
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        for (path, content, mode) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(*mode);
            header.set_entry_type(tar::EntryType::Regular);
            // Written by hand, so the checks see paths `append_data` would refuse.
            header.as_gnu_mut().unwrap().name[..path.len()].copy_from_slice(path.as_bytes());
            header.set_cksum();
            builder.append(&header, *content).unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap()
    }

    #[test]
    fn checks_the_checksum() {
        let sha = hex::encode(Sha256::digest(b"plugin"));
        assert!(checksum_matches(b"plugin", &sha));
        assert!(checksum_matches(b"plugin", &sha.to_uppercase()));
        assert!(!checksum_matches(b"tampered", &sha));
    }

    #[test]
    fn unpacks_only_inside_the_plugin() {
        let dir = tempfile::tempdir().unwrap();
        let ok = archive(&[
            ("qif/pennychest-plugin.toml", b"name", 0o644),
            ("qif/run", b"#!", 0o4777),
        ]);
        unpack(&ok, dir.path()).unwrap();
        assert_eq!(plugin_root(dir.path()).unwrap(), dir.path().join("qif"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(dir.path().join("qif/run"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o7777, 0o755, "no setuid, nothing world-writable");
        }

        for path in ["../escape", "/etc/passwd", "a/../../b"] {
            let dir = tempfile::tempdir().unwrap();
            let error = unpack(&archive(&[(path, b"x", 0o644)]), dir.path()).unwrap_err();
            assert!(
                error.to_string().contains("outside the plugin"),
                "{path}: {error}"
            );
        }
    }

    #[test]
    fn wants_a_manifest_at_the_top() {
        let dir = tempfile::tempdir().unwrap();
        unpack(
            &archive(&[("a/b/pennychest-plugin.toml", b"", 0o644)]),
            dir.path(),
        )
        .unwrap();
        assert!(plugin_root(dir.path()).is_err());
    }
}

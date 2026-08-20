//! Source-tree walking shared by every extractor.
//!
//! Every listing this module returns is sorted case-insensitively by path, because
//! extraction order decides document order inside aggregate documents and therefore
//! whether a re-run reports zero changed files.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::Result;
use walkdir::WalkDir;

use crate::reference::slug;

/// Every `.xdb` below `root`, in a stable order. A missing directory yields nothing.
pub(crate) fn sorted_xdb_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files: Vec<PathBuf> = WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(|entry| match entry {
            Ok(entry)
                if entry.file_type().is_file()
                    && entry.path().extension().is_some_and(|extension| {
                        extension.to_string_lossy().eq_ignore_ascii_case("xdb")
                    }) =>
            {
                Some(Ok(entry.into_path()))
            }
            Ok(_) => None,
            Err(error) => Some(Err(anyhow::Error::from(error))),
        })
        .collect::<Result<_>>()?;
    sort_paths(&mut files);
    Ok(files)
}

pub(crate) fn sort_paths(paths: &mut [PathBuf]) {
    paths.sort_by_cached_key(|path| path.to_string_lossy().to_ascii_lowercase());
}

/// Find a child directory of `root` whose name matches `name` exactly or by slug.
pub(crate) fn find_named_directory(root: &Path, name: &str) -> Option<PathBuf> {
    let requested_slug = slug(name);
    fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .find_map(|entry| {
            let is_dir = entry.file_type().ok()?.is_dir();
            let entry_name = entry.file_name().to_string_lossy().into_owned();
            (is_dir
                && (entry_name.eq_ignore_ascii_case(name) || slug(&entry_name) == requested_slug))
                .then(|| entry.path())
        })
}

/// The `Characters/<family>/Instances/<zone>` and `Creatures/<family>/Instances/<zone>`
/// directories that hold one zone's mob instances, resolved to their `.xdb` files.
pub(crate) fn zone_instance_files(src: &Path, zone: &str) -> Result<Vec<PathBuf>> {
    let mut directories = Vec::new();
    for family_root in [src.join("Characters"), src.join("Creatures")] {
        if !family_root.is_dir() {
            continue;
        }
        for entry in fs::read_dir(family_root)? {
            let entry = entry?;
            if entry.file_type()?.is_dir()
                && let Some(directory) = find_named_directory(&entry.path().join("Instances"), zone)
            {
                directories.push(directory);
            }
        }
    }
    directories.sort();
    let mut files = Vec::new();
    for directory in directories {
        files.extend(sorted_xdb_files(&directory)?);
    }
    sort_paths(&mut files);
    Ok(files)
}

/// Resolve one `href` against the source root and the file that carried it.
///
/// Hrefs come in three shapes: rooted (`/World/Factions/Wild.xdb`), relative to the
/// referring file (`Common.xdb`, `../Shared/Common.xdb`), and either shape with an
/// `#xpointer(...)` fragment. Returns the absolute path plus the root-relative path
/// used to mint canonical ids.
pub(crate) fn resolve_href(src: &Path, referrer: &Path, href: &str) -> Option<(PathBuf, String)> {
    let target = href.split('#').next()?.trim();
    if target.is_empty() {
        return None;
    }
    let normalized = target.replace('\\', "/");
    let path = if let Some(rooted) = normalized.strip_prefix('/') {
        src.join(rooted)
    } else {
        referrer.parent()?.join(&normalized)
    };
    let cleaned = normalize(&path);
    let relative = cleaned
        .strip_prefix(src)
        .ok()?
        .components()
        .map(|part| part.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    Some((cleaned, relative))
}

/// The canonical localization key one `loc_ref` href names.
///
/// A raw href is either relative to the file that carries it or rooted at the data
/// root, so it is ambiguous on its own and a rooted one does not even satisfy the
/// schema's key pattern. The key is the root-relative path with the `.txt` dropped,
/// which is exactly what the locale extractor writes on the other side of the join.
pub(crate) fn loc_key(src: &Path, referrer: &Path, href: &str) -> Option<String> {
    let (_, relative) = resolve_href(src, referrer, href)?;
    Some(strip_txt(&relative).to_owned())
}

pub(crate) fn strip_txt(relative: &str) -> &str {
    relative
        .len()
        .checked_sub(4)
        .filter(|index| relative[*index..].eq_ignore_ascii_case(".txt"))
        .map_or(relative, |index| &relative[..index])
}

/// Collapse `.` and `..` textually. The source trees hold no symlinks, so this needs
/// no filesystem access and works for paths that do not exist yet.
fn normalize(path: &Path) -> PathBuf {
    let mut parts: Vec<std::path::Component<'_>> = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if matches!(parts.last(), Some(std::path::Component::Normal(_))) {
                    parts.pop();
                } else {
                    parts.push(component);
                }
            }
            other => parts.push(other),
        }
    }
    parts.iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_rooted_relative_and_parent_hrefs() {
        let src = Path::new("/src");
        let referrer = Path::new("/src/Mechanics/MobQualities/Common.xdb");
        assert_eq!(
            resolve_href(src, referrer, "/World/Factions/Wild.xdb#xpointer(/Faction)")
                .unwrap()
                .1,
            "World/Factions/Wild.xdb"
        );
        assert_eq!(
            resolve_href(src, referrer, "QualityPrototype.xdb#x")
                .unwrap()
                .1,
            "Mechanics/MobQualities/QualityPrototype.xdb"
        );
        assert_eq!(
            resolve_href(src, referrer, "../MobClasses/Default.xdb")
                .unwrap()
                .1,
            "Mechanics/MobClasses/Default.xdb"
        );
        assert_eq!(resolve_href(src, referrer, ""), None);
    }
}

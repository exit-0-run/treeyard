//! Input safety: repo paths that are safe to write on disk, tree checks for hostile repo
//! content, and the size limits of the API. Every place that turns a repo path into a file
//! (workspaces, test runs, bisect, regenerate) goes through `dest`.

use anyhow::{Result, bail};
use std::path::{Path, PathBuf};

/// Longest repo path in bytes (git itself has no limit; Linux PATH_MAX is 4096).
pub const MAX_PATH: usize = 4096;
/// Longest path component in bytes (NAME_MAX on Linux and macOS).
pub const MAX_COMPONENT: usize = 255;
/// Most edits in one `apply` call.
pub const MAX_EDITS: usize = 10_000;
/// Largest file an agent may write through the API (the HTTP body limit is 64 MiB).
pub const MAX_FILE: usize = 48 << 20;
/// Largest file `read` returns inline; bigger files go through `blobs` or git clone.
pub const MAX_READ: usize = 32 << 20;

/// Code points that HFS+ (macOS) ignores in names, so ".g\u{200c}it" opens ".git"
/// (the list git uses in `is_hfs_dotgit`, utf8.c).
fn hfs_ignorable(c: char) -> bool {
    matches!(c, '\u{200c}'..='\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{206a}'..='\u{206f}' | '\u{feff}')
}

/// Whether a path component names the git directory on some file system: ".git" in any case,
/// with HFS+ ignorable code points, or with the trailing dots and spaces that NTFS drops,
/// or the NTFS short name "git~1".
pub fn is_dotgit(component: &str) -> bool {
    let c: String = component.chars().filter(|c| !hfs_ignorable(*c)).collect();
    let c = c.trim_end_matches(['.', ' ']);
    c.eq_ignore_ascii_case(".git") || c.eq_ignore_ascii_case("git~1")
}

/// Checks a repo path: relative, `/`-separated, no empty, `.` or `..` component, no git
/// directory, no NUL or backslash, within the length limits.
pub fn check_path(path: &str) -> Result<()> {
    let why = if path.is_empty() {
        Some("it is empty")
    } else if path.len() > MAX_PATH {
        Some("it is longer than 4096 bytes")
    } else if path.contains('\0') {
        Some("it contains NUL")
    } else if path.contains('\\') {
        Some("it contains a backslash")
    } else if path.starts_with('/') {
        Some("it is absolute")
    } else if path.split('/').any(|c| c.is_empty() || c == "." || c == "..") {
        Some("it has an empty, . or .. component")
    } else if path.split('/').any(is_dotgit) {
        Some("it names a .git directory")
    } else if path.split('/').any(|c| c.len() > MAX_COMPONENT) {
        Some("a component is longer than 255 bytes")
    } else {
        None
    };
    match why {
        Some(why) => bail!("invalid path {:?}: {why}; use a relative path like src/main.rs", clip(path)),
        None => Ok(()),
    }
}

fn clip(path: &str) -> String {
    if path.len() <= 200 {
        return path.to_string();
    }
    let mut end = 200;
    while !path.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &path[..end])
}

/// Checks every path of a tree (sorted or not) and that no file sits where a directory of
/// another path is: such a tree would let a symlink redirect a later write.
pub fn check_tree<'a>(paths: impl IntoIterator<Item = &'a str>) -> Result<()> {
    let all: std::collections::BTreeSet<&str> = paths.into_iter().collect();
    for p in &all {
        check_path(p)?;
        for (i, _) in p.match_indices('/') {
            if all.contains(&p[..i]) {
                bail!("hostile tree: {:?} sits under the file {:?}", clip(p), clip(&p[..i]));
            }
        }
    }
    Ok(())
}

/// The file for repo path `path` under `root`, after `check_path`. Every directory between
/// `root` and the file that already exists must be a real directory, never a symlink, so a
/// symlink in the tree cannot send the write outside `root`.
pub fn dest(root: &Path, path: &str) -> Result<PathBuf> {
    check_path(path)?;
    let mut cur = root.to_path_buf();
    let comps: Vec<&str> = path.split('/').collect();
    for c in &comps[..comps.len() - 1] {
        cur.push(c);
        match std::fs::symlink_metadata(&cur) {
            Ok(m) if m.file_type().is_symlink() => {
                bail!("refusing to write {:?}: {:?} is a symlink", clip(path), cur.display().to_string())
            }
            Ok(m) if !m.is_dir() => bail!("refusing to write {:?}: {} is a file", clip(path), cur.display()),
            _ => {}
        }
    }
    let file = cur.join(comps[comps.len() - 1]);
    if std::fs::symlink_metadata(&file).is_ok_and(|m| m.file_type().is_symlink()) {
        // Replace the link itself; never write through it.
        std::fs::remove_file(&file)?;
    }
    Ok(file)
}

/// Whether `addr` (host:port, as given to `serve`) only listens on the loopback interface.
pub fn is_loopback(addr: &str) -> bool {
    let host = match addr.rsplit_once(':') {
        Some((h, _)) => h,
        None => addr,
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostile_paths_are_refused() {
        for p in [
            "",
            "/etc/passwd",
            "../x",
            "a/../../x",
            "a//b",
            "./a",
            "a/.",
            ".git/config",
            ".GIT/hooks/post-checkout",
            "sub/.Git/HEAD",
            ".g\u{200c}it/config",
            ".git./x",
            ".git /x",
            "GIT~1/config",
            "a\0b",
            "a\\..\\b",
        ] {
            assert!(check_path(p).is_err(), "{p:?} must be refused");
        }
        let long = "a/".repeat(2100) + "b";
        assert!(check_path(&long).is_err());
        assert!(check_path(&"x".repeat(256)).is_err());
        for p in ["src/main.rs", ".github/x.yml", ".gitignore", "a/.gitmodules", "dir.git/x", "ünï/çødé.txt", "a b/c d"]
        {
            check_path(p).unwrap_or_else(|e| panic!("{p:?} must be allowed: {e}"));
        }
        let deep = (0..200).map(|i| format!("d{i}")).collect::<Vec<_>>().join("/");
        check_path(&deep).unwrap();
    }

    #[test]
    fn tree_with_a_file_over_a_directory_is_refused() {
        assert!(check_tree(["link", "link/passwd"]).is_err());
        check_tree(["a/b", "a/c", "ab"]).unwrap();
    }

    #[test]
    fn dest_never_writes_through_a_symlink() {
        let tmp = tempfile::tempdir().unwrap();
        let (root, outside) = (tmp.path().join("root"), tmp.path().join("outside"));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
        assert!(dest(&root, "link/x").is_err());
        // A symlink at the file itself is replaced, not followed.
        std::os::unix::fs::symlink(outside.join("f"), root.join("f")).unwrap();
        let f = dest(&root, "f").unwrap();
        std::fs::write(&f, "x").unwrap();
        assert!(!outside.join("f").exists());
    }

    #[test]
    fn loopback_addresses() {
        for a in ["127.0.0.1:7420", "localhost:1", "[::1]:7420", "127.1.2.3:80"] {
            assert!(is_loopback(a), "{a}");
        }
        for a in ["0.0.0.0:7420", "[::]:7420", "192.168.1.5:7420", "example.com:80"] {
            assert!(!is_loopback(a), "{a}");
        }
    }
}

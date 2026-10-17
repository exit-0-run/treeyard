//! Copy-on-write directory clones for workspaces and dependency layers.
//!
//! `clone`: one APFS `clonefile(2)` call clones a whole directory (macOS).
//! `reflink`: walk the tree and clone each file (`FICLONE` on btrfs/XFS, `clonefile` per file on APFS).
//! `overlay`: mount overlayfs with the base as the lower layer (Linux, needs mount rights).
//! `hardlink`: link each file to the base and make the base read-only (in-place writers fail).
//! `copy`: plain copy, the last fallback. `auto` picks the first one the file system supports.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Strategy {
    Auto,
    Clone,
    Reflink,
    Overlay,
    Hardlink,
    Copy,
}

impl std::str::FromStr for Strategy {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        Ok(match s {
            "auto" => Strategy::Auto,
            "clone" => Strategy::Clone,
            "reflink" => Strategy::Reflink,
            "overlay" => Strategy::Overlay,
            "hardlink" => Strategy::Hardlink,
            "copy" => Strategy::Copy,
            _ => bail!("unknown strategy {s:?}: auto, clone, reflink, overlay, hardlink or copy"),
        })
    }
}

impl Strategy {
    pub fn as_str(self) -> &'static str {
        match self {
            Strategy::Auto => "auto",
            Strategy::Clone => "clone",
            Strategy::Reflink => "reflink",
            Strategy::Overlay => "overlay",
            Strategy::Hardlink => "hardlink",
            Strategy::Copy => "copy",
        }
    }
}

/// True when the error means "this file system cannot clone".
fn unsupported(e: &io::Error) -> bool {
    matches!(e.raw_os_error(), Some(libc::ENOTSUP) | Some(libc::EXDEV) | Some(libc::EINVAL) | Some(libc::ENOSYS))
        || e.raw_os_error() == Some(libc::EOPNOTSUPP)
}

#[cfg(target_os = "macos")]
fn cstr(p: &Path) -> Result<std::ffi::CString> {
    use std::os::unix::ffi::OsStrExt;
    Ok(std::ffi::CString::new(p.as_os_str().as_bytes())?)
}

/// Clones `src` (a file or a whole directory) to `dst` with one `clonefile` call.
#[cfg(target_os = "macos")]
fn clonefile(src: &Path, dst: &Path) -> io::Result<()> {
    let (s, d) = (cstr(src).map_err(io::Error::other)?, cstr(dst).map_err(io::Error::other)?);
    // CLONE_NOFOLLOW: clone a symlink itself, never its target.
    const CLONE_NOFOLLOW: u32 = 0x0001; // sys/clonefile.h
    let rc = unsafe { libc::clonefile(s.as_ptr(), d.as_ptr(), CLONE_NOFOLLOW) };
    if rc == 0 { Ok(()) } else { Err(io::Error::last_os_error()) }
}

#[cfg(not(target_os = "macos"))]
fn clonefile(_src: &Path, _dst: &Path) -> io::Result<()> {
    Err(io::Error::from_raw_os_error(libc::ENOTSUP))
}

/// Clones one regular file's data blocks (reflink), keeping permissions.
fn reflink_file(src: &Path, dst: &Path) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        clonefile(src, dst)
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::io::AsRawFd;
        let from = fs::File::open(src)?;
        let to = fs::File::create(dst)?;
        // FICLONE = _IOW(0x94, 9, int)
        const FICLONE: libc::c_ulong = 0x4004_9409;
        let rc = unsafe { libc::ioctl(to.as_raw_fd(), FICLONE as _, from.as_raw_fd()) };
        if rc != 0 {
            let e = io::Error::last_os_error();
            drop(to);
            let _ = fs::remove_file(dst);
            return Err(e);
        }
        fs::set_permissions(dst, from.metadata()?.permissions())
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (src, dst);
        Err(io::Error::from_raw_os_error(libc::ENOTSUP))
    }
}

fn copy_file(src: &Path, dst: &Path) -> io::Result<()> {
    fs::copy(src, dst).map(|_| ())
}

/// Copies modification time, so stat caches made for `src` stay valid for `dst`.
fn keep_mtime(src: &fs::Metadata, dst: &Path) -> io::Result<()> {
    fs::File::options().write(true).open(dst)?.set_modified(src.modified()?)
}

/// Walks `src` and recreates it at `dst`, cloning each file with `file`.
fn walk(src: &Path, dst: &Path, file: &dyn Fn(&Path, &Path, &fs::Metadata) -> io::Result<()>) -> io::Result<()> {
    let meta = fs::symlink_metadata(src)?;
    if meta.is_dir() {
        fs::create_dir(dst)?;
        for e in fs::read_dir(src)? {
            let e = e?;
            walk(&e.path(), &dst.join(e.file_name()), file)?;
        }
        fs::set_permissions(dst, meta.permissions())
    } else if meta.file_type().is_symlink() {
        std::os::unix::fs::symlink(fs::read_link(src)?, dst)
    } else {
        file(src, dst, &meta)
    }
}

/// Clones `src` to `dst` (which must not exist) and returns the strategy used.
/// `Overlay` is handled by `overlay_mount`; here it is an error.
pub fn clone_tree(src: &Path, dst: &Path, strategy: Strategy) -> Result<Strategy> {
    if fs::symlink_metadata(dst).is_ok() {
        bail!("{} already exists", dst.display());
    }
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    let ctx = || format!("clone {} to {}", src.display(), dst.display());
    match strategy {
        Strategy::Auto => {
            for s in [Strategy::Clone, Strategy::Reflink] {
                match try_clone(src, dst, s) {
                    Ok(()) => return Ok(s),
                    Err(e) if unsupported(&e) => remove_tree(dst)?,
                    Err(e) => return Err(e).with_context(ctx),
                }
            }
            try_clone(src, dst, Strategy::Copy).with_context(ctx)?;
            Ok(Strategy::Copy)
        }
        Strategy::Overlay => bail!("overlay workspaces are mounted, not cloned"),
        s => {
            try_clone(src, dst, s).with_context(ctx)?;
            Ok(s)
        }
    }
}

fn try_clone(src: &Path, dst: &Path, s: Strategy) -> io::Result<()> {
    match s {
        Strategy::Clone => clonefile(src, dst),
        Strategy::Reflink => walk(src, dst, &|a, b, m| {
            reflink_file(a, b)?;
            keep_mtime(m, b)
        }),
        Strategy::Hardlink => walk(src, dst, &|a, b, _| fs::hard_link(a, b)),
        _ => walk(src, dst, &|a, b, m| {
            copy_file(a, b)?;
            keep_mtime(m, b)
        }),
    }
}

/// Makes every regular file under `dir` read-only (a base used by hardlink workspaces).
pub fn make_read_only(dir: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    for e in walk_files(dir)? {
        let m = fs::symlink_metadata(&e)?;
        if m.is_file() {
            let mode = m.permissions().mode() & !0o222;
            fs::set_permissions(&e, fs::Permissions::from_mode(mode))?;
        }
    }
    Ok(())
}

fn walk_files(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d)? {
            let e = e?;
            if e.file_type()?.is_dir() {
                stack.push(e.path());
            } else {
                out.push(e.path());
            }
        }
    }
    Ok(out)
}

/// Removes a tree, also when its files or directories are read-only.
pub fn remove_tree(dir: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let Ok(meta) = fs::symlink_metadata(dir) else { return Ok(()) };
    if !meta.is_dir() {
        return Ok(fs::remove_file(dir)?);
    }
    if fs::remove_dir_all(dir).is_ok() {
        return Ok(());
    }
    // Read-only directories block deletion of their entries: open them up, then retry.
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        fs::set_permissions(&d, fs::Permissions::from_mode(0o755))?;
        for e in fs::read_dir(&d)? {
            let e = e?;
            if e.file_type()?.is_dir() {
                stack.push(e.path());
            }
        }
    }
    fs::remove_dir_all(dir).with_context(|| format!("remove {}", dir.display()))
}

/// The mount options for an overlayfs workspace (pure, so it is tested on every platform).
pub fn overlay_options(lower: &Path, upper: &Path, work: &Path) -> String {
    format!("lowerdir={},upperdir={},workdir={}", lower.display(), upper.display(), work.display())
}

/// Mounts `dst` as an overlay of `lower`; writes go to `<dst>.upper`.
pub fn overlay_mount(lower: &Path, dst: &Path) -> Result<()> {
    if !cfg!(target_os = "linux") {
        bail!("overlay workspaces need Linux (overlayfs); use clone, reflink or copy here");
    }
    let (upper, work) = overlay_dirs(dst);
    for d in [dst, &upper, &work] {
        fs::create_dir_all(d)?;
    }
    let out = std::process::Command::new("mount")
        .args(["-t", "overlay", "overlay", "-o", &overlay_options(lower, &upper, &work)])
        .arg(dst)
        .output()
        .context("run mount")?;
    if !out.status.success() {
        let _ = fs::remove_dir(dst);
        bail!(
            "mount -t overlay failed (needs root or a user namespace): {}",
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(())
}

pub fn overlay_dirs(dst: &Path) -> (PathBuf, PathBuf) {
    let name = dst.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    (dst.with_file_name(format!("{name}.upper")), dst.with_file_name(format!("{name}.work")))
}

pub fn overlay_unmount(dst: &Path) -> Result<()> {
    let out = std::process::Command::new("umount").arg(dst).output().context("run umount")?;
    if !out.status.success() {
        bail!("umount {}: {}", dst.display(), String::from_utf8_lossy(&out.stderr).trim());
    }
    let (upper, work) = overlay_dirs(dst);
    for d in [upper, work] {
        remove_tree(&d)?;
    }
    let _ = fs::remove_dir(dst);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn sample(root: &Path) -> PathBuf {
        let src = root.join("src");
        fs::create_dir_all(src.join("a/b")).unwrap();
        fs::write(src.join("top.txt"), "top\n").unwrap();
        fs::write(src.join("a/b/deep.txt"), "deep\n").unwrap();
        fs::write(src.join("run.sh"), "#!/bin/sh\n").unwrap();
        fs::set_permissions(src.join("run.sh"), fs::Permissions::from_mode(0o755)).unwrap();
        std::os::unix::fs::symlink("top.txt", src.join("link")).unwrap();
        src
    }

    fn check(src: &Path, dst: &Path) {
        assert_eq!(fs::read_to_string(dst.join("a/b/deep.txt")).unwrap(), "deep\n");
        assert_eq!(fs::read_link(dst.join("link")).unwrap(), Path::new("top.txt"));
        assert_eq!(fs::metadata(dst.join("run.sh")).unwrap().permissions().mode() & 0o111, 0o111);
        let (a, b) = (fs::metadata(src.join("top.txt")).unwrap(), fs::metadata(dst.join("top.txt")).unwrap());
        assert_eq!(a.modified().unwrap(), b.modified().unwrap(), "mtime kept");
    }

    #[test]
    fn every_strategy_clones_and_keeps_the_source() {
        let tmp = tempfile::tempdir().unwrap();
        let src = sample(tmp.path());
        for s in [Strategy::Auto, Strategy::Reflink, Strategy::Copy, Strategy::Clone] {
            let dst = tmp.path().join(format!("dst-{}", s.as_str()));
            match clone_tree(&src, &dst, s) {
                Ok(used) => {
                    check(&src, &dst);
                    fs::write(dst.join("top.txt"), "changed\n").unwrap();
                    assert_eq!(fs::read_to_string(src.join("top.txt")).unwrap(), "top\n", "{used:?} leaked a write");
                }
                // Clone and reflink exist only on file systems that support them.
                Err(e) => assert!(matches!(s, Strategy::Clone | Strategy::Reflink), "{s:?}: {e:#}"),
            }
        }
    }

    #[test]
    fn hardlink_shares_files_with_a_read_only_base() {
        let tmp = tempfile::tempdir().unwrap();
        let src = sample(tmp.path());
        make_read_only(&src).unwrap();
        let dst = tmp.path().join("dst");
        clone_tree(&src, &dst, Strategy::Hardlink).unwrap();
        check(&src, &dst);
        // In-place writes fail on the shared inode (root ignores the mode bits, so skip it there);
        // replace-by-rename (what sed -i does) works.
        if unsafe { libc::geteuid() } != 0 {
            assert!(fs::write(dst.join("top.txt"), "x").is_err());
        }
        fs::write(dst.join("top.txt.new"), "changed\n").unwrap();
        fs::rename(dst.join("top.txt.new"), dst.join("top.txt")).unwrap();
        assert_eq!(fs::read_to_string(src.join("top.txt")).unwrap(), "top\n");
        remove_tree(&src).unwrap();
        assert!(!src.exists());
    }

    #[test]
    fn overlay_options_name_the_three_dirs() {
        let (u, w) = overlay_dirs(Path::new("/n/ws/a"));
        assert_eq!(u, Path::new("/n/ws/a.upper"));
        assert_eq!(
            overlay_options(Path::new("/n/bases/x"), &u, &w),
            "lowerdir=/n/bases/x,upperdir=/n/ws/a.upper,workdir=/n/ws/a.work"
        );
    }

    #[test]
    fn auto_uses_a_clone_on_apfs() {
        let tmp = tempfile::tempdir().unwrap();
        let src = sample(tmp.path());
        let used = clone_tree(&src, &tmp.path().join("dst"), Strategy::Auto).unwrap();
        if cfg!(target_os = "macos") {
            // macOS temp dirs are on APFS, so the one-call directory clone must work.
            assert_eq!(used, Strategy::Clone);
        }
    }

    /// Runs where the temp dir is on btrfs or XFS with reflink (set ROOTSTOCK_EXPECT_REFLINK=1).
    #[cfg(target_os = "linux")]
    #[test]
    fn reflink_on_a_reflink_file_system() {
        if std::env::var_os("ROOTSTOCK_EXPECT_REFLINK").is_none() {
            eprintln!("skip: set ROOTSTOCK_EXPECT_REFLINK=1 with TMPDIR on btrfs or XFS (reflink=1)");
            return;
        }
        let tmp = tempfile::tempdir().unwrap();
        let src = sample(tmp.path());
        assert_eq!(clone_tree(&src, &tmp.path().join("r"), Strategy::Reflink).unwrap(), Strategy::Reflink);
        check(&src, &tmp.path().join("r"));
        assert_eq!(clone_tree(&src, &tmp.path().join("a"), Strategy::Auto).unwrap(), Strategy::Reflink);
    }
}

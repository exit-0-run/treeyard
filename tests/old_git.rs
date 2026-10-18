//! git before 2.44 has no `merge-file --diff-algorithm` (Ubuntu 24.04 ships 2.43): line merges must still
//! work. A wrapper `git` first on PATH rejects the option the way git 2.43 does. Its own test binary,
//! because it changes PATH for the whole process.

use std::os::unix::fs::PermissionsExt;

#[test]
fn line_merges_work_without_merge_file_diff_algorithm() {
    let dir = tempfile::tempdir().unwrap();
    let real =
        String::from_utf8(std::process::Command::new("sh").args(["-c", "command -v git"]).output().unwrap().stdout)
            .unwrap();
    let wrapper = dir.path().join("git");
    std::fs::write(
        &wrapper,
        format!(
            "#!/bin/sh\nfor a in \"$@\"; do [ \"$a\" = --diff-algorithm ] && {{ echo \"error: unknown option \\`diff-algorithm'\" >&2; exit 129; }}; done\nexec {} \"$@\"\n",
            real.trim()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&wrapper, std::fs::Permissions::from_mode(0o755)).unwrap();
    // SAFETY: the only test in this binary, set before any thread reads the environment.
    unsafe { std::env::set_var("PATH", format!("{}:{}", dir.path().display(), std::env::var("PATH").unwrap())) };

    let tmp = dir.path().join("merges");
    let base = "a\nb\nc\nd\ne\n";
    for _ in 0..2 {
        let clean = treeyard::merge::git_merge_file(&tmp, base, "a\nB\nc\nd\ne\n", "a\nb\nc\nd\nE\n").unwrap();
        assert_eq!(clean, Ok("a\nB\nc\nd\nE\n".to_string()));
    }
    let conflict = treeyard::merge::git_merge_file(&tmp, base, "a\nX\nc\nd\ne\n", "a\nY\nc\nd\ne\n").unwrap();
    let text = conflict.unwrap_err();
    assert!(text.contains("<<<<<<< main") && text.contains(">>>>>>> change"), "{text}");
}

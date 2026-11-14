use std::process::Command;

#[cfg(unix)]
#[test]
fn sudo_keeps_the_original_accounts_home_protected() {
    use nix::unistd::{Uid, User};

    let uid = std::env::var("SUDO_UID")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
        .unwrap_or_else(|| Uid::current().as_raw());
    let user = User::from_uid(Uid::from_raw(uid)).unwrap().unwrap();
    // run sudo as the original account even when the test runner itself was elevated.
    let output = Command::new("sudo")
        .args(["-n", "-u", &format!("#{uid}"), "sudo", "-n"])
        .arg(env!("CARGO_BIN_EXE_mudo"))
        .args(["apt", "opt", "gop", "stop", "remove", "--dry-run", "-r"])
        .arg(&user.dir)
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("protected system path"));
    assert!(user.dir.is_dir());
}

#[cfg(unix)]
#[test]
fn sudo_requires_the_full_command_for_help_and_version() {
    let bin = env!("CARGO_BIN_EXE_mudo");
    for flag in ["--help", "--version"] {
        let output = Command::new("sudo")
            .args(["-n", bin, "apt", "opt", "gop", "stop", "remove", flag])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(0));
        assert!(!output.stdout.is_empty());
    }
    for args in [
        vec!["--help"],
        vec!["apt", "opt", "gop", "stop", "--version"],
        vec!["apt", "opt", "gop", "stop", "wrong", "--help"],
        vec!["opt", "apt", "gop", "stop", "remove", "--version"],
        vec!["apt", "opt", "gop", "stop", "remov", "--help"],
    ] {
        let output = Command::new("sudo")
            .args(["-n", bin])
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
    }
}

#[test]
fn direct_invocation_refuses_help_version_and_erase() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("kept.txt");
    std::fs::write(&path, b"kept").unwrap();
    for args in [
        vec!["apt", "opt", "gop", "stop", "remove", "--help"],
        vec!["apt", "opt", "gop", "stop", "remove", "--version"],
        vec![
            "apt",
            "opt",
            "gop",
            "stop",
            "remove",
            "-f",
            path.to_str().unwrap(),
        ],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_mudo"))
            .env_remove("SUDO_UID")
            .env_remove("SUDO_GID")
            .env_remove("SUDO_USER")
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1));
        assert!(String::from_utf8_lossy(&output.stderr).contains("run with sudo"));
        assert_eq!(std::fs::read(&path).unwrap(), b"kept");
    }
}

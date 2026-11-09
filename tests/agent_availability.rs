#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::{Command, Output, Stdio};

fn executable(path: &Path) {
    fs::write(path, "#!/bin/sh\nexit 99\n").unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn edgee(home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_edgee"))
        .args(args)
        .env_clear()
        .env("HOME", home)
        .env("CODEX_HOME", home.join(".codex"))
        .env(
            "PATH",
            std::env::join_paths([home.join(".edgee/bin"), home.join("bin")]).unwrap(),
        )
        .env("EDGEE_NO_UPDATE_CHECK", "1")
        .current_dir(home)
        .stdin(Stdio::null())
        .output()
        .unwrap()
}

#[test]
fn missing_agents_fail_before_onboarding_even_with_an_edgee_alias_and_reroute() {
    let home = tempfile::tempdir().unwrap();
    let shims = home.path().join(".edgee/bin");
    fs::create_dir_all(&shims).unwrap();
    let agents = [
        ("claude", "claude", "Claude Code"),
        ("codex", "codex", "Codex CLI"),
        ("codebuddy", "codebuddy", "CodeBuddy"),
        ("opencode", "opencode", "OpenCode"),
        ("crush", "crush", "Crush"),
        ("pi", "pi", "Pi"),
        ("omp", "omp", "OMP"),
        ("deepseek", "dsh", "DeepSeek Harness"),
        ("kimi", "kimi", "Kimi Code"),
        ("kilo", "kilo", "Kilo Code"),
        ("copilot-cli", "copilot", "GitHub Copilot CLI"),
    ];
    for (target, binary, label) in agents {
        executable(&shims.join(binary));
        for args in [
            vec!["launch", target],
            vec!["launch", "--reroute", "openai/gpt-5", target],
        ] {
            let result = edgee(home.path(), &args);
            let stderr = String::from_utf8_lossy(&result.stderr);
            assert!(!result.status.success(), "{target} should not launch");
            assert!(
                stderr.contains(&format!("{label} is not installed")),
                "{target}: {stderr}"
            );
            assert!(stderr.contains("Install it"), "{target}: {stderr}");
            assert!(!home.path().join(".edgee/credentials.toml").exists());
            assert!(
                !home.path().join(".codex").exists(),
                "Codex setup ran before the binary check"
            );
        }
    }
}

#[test]
fn aliases_are_installed_for_missing_agents_but_only_real_binaries_are_displayed() {
    let home = tempfile::tempdir().unwrap();
    let bin = home.path().join("bin");
    fs::create_dir(&bin).unwrap();
    let alias = home.path().join(".edgee/bin/kimi");

    // Installing the shim must not make an absent agent look installed.
    for _ in 0..2 {
        let result = edgee(home.path(), &["alias", "kimi"]);
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        assert!(alias.is_file());
        assert!(!String::from_utf8_lossy(&result.stdout).contains("Terminal aliases"));
    }

    executable(&bin.join("kimi"));
    let result = edgee(home.path(), &["alias", "kimi"]);
    assert!(result.status.success());
    let stdout = String::from_utf8_lossy(&result.stdout);
    assert!(stdout.contains("Terminal aliases · 1"));
    assert!(stdout.contains("kimi"));

    let result = edgee(home.path(), &["alias", "remove", "kimi"]);
    assert!(result.status.success());
    assert!(String::from_utf8_lossy(&result.stdout).contains("kimi"));
    assert!(!alias.exists());

    fs::remove_file(bin.join("kimi")).unwrap();
    let result = edgee(home.path(), &["alias", "remove", "kimi"]);
    assert!(result.status.success());
    assert!(!String::from_utf8_lossy(&result.stdout).contains("Terminal aliases"));
}

#[test]
fn alias_removal_continues_when_codex_config_cannot_be_repaired() {
    for unreadable in [false, true] {
        let home = tempfile::tempdir().unwrap();
        for agent in ["kimi", "codex"] {
            let result = edgee(home.path(), &["alias", agent]);
            assert!(
                result.status.success(),
                "{}",
                String::from_utf8_lossy(&result.stderr)
            );
        }
        let config = home.path().join(".codex/config.toml");
        fs::create_dir_all(config.parent().unwrap()).unwrap();
        if unreadable {
            fs::create_dir(&config).unwrap();
        } else {
            fs::write(&config, "model = [").unwrap();
        }

        let result = edgee(home.path(), &["alias", "remove", "all"]);
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(result.status.success(), "{stderr}");
        assert!(stderr.contains("Could not configure Codex"), "{stderr}");
        assert!(stderr.contains("Alias removal will continue"), "{stderr}");
        for agent in ["kimi", "codex"] {
            assert!(!home.path().join(".edgee/bin").join(agent).exists());
        }
        if unreadable {
            assert!(config.is_dir());
        } else {
            assert_eq!(fs::read_to_string(config).unwrap(), "model = [");
        }
    }
}

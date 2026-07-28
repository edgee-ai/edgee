pub fn detect_origin() -> Option<String> {
    get_remote_url("origin").or_else(|| {
        // fall back to the first configured remote
        let output = std::process::Command::new("git")
            .args(["remote"])
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let first = String::from_utf8(output.stdout)
            .ok()?
            .lines()
            .next()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)?;
        get_remote_url(&first)
    })
}

fn get_remote_url(remote: &str) -> Option<String> {
    std::process::Command::new("git")
        .args(["remote", "get-url", remote])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

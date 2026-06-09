//! Bash command output compressors.
//!
//! Commands are grouped by ecosystem:
//! - `fs`     — ls, find, tree, grep, rg
//! - `vcs`    — diff, git, gh
//! - `rust`   — cargo
//! - `python` — pytest, mypy, ruff
//! - `js`     — npm, jest, tsc, eslint
//! - `go`     — go, golangci-lint
//! - `sys`    — docker, env, curl, make, psql

mod fs;
mod go;
mod js;
mod python;
mod rust;
mod sys;
mod vcs;

/// Trait for compressing the output of a specific bash command.
pub trait BashCompressor {
    /// Compress the output of a command.
    /// Returns `Some(compressed)` if compression was applied, `None` to leave as-is.
    fn compress(&self, command: &str, output: &str) -> Option<String>;
}

/// Select the appropriate compressor for a base command (e.g. "ls", "find").
/// Returns `None` for commands we don't compress.
pub fn compressor_for(base_command: &str) -> Option<&'static dyn BashCompressor> {
    match base_command {
        "ls" => Some(&fs::LsCompressor),
        "tree" => Some(&fs::TreeCompressor),
        "find" => Some(&fs::FindCompressor),
        "grep" => Some(&fs::GrepCompressor),
        "rg" => Some(&fs::RgCompressor),
        "diff" => Some(&vcs::DiffCompressor),
        "git" => Some(&vcs::GitCompressor),
        "gh" => Some(&vcs::GhCompressor),
        "cargo" => Some(&rust::CargoCompressor),
        "docker" => Some(&sys::DockerCompressor),
        "env" | "printenv" => Some(&sys::EnvCompressor),
        "npm" | "pnpm" | "npx" => Some(&js::NpmCompressor),
        "pytest" | "python" => Some(&python::PytestCompressor),
        "psql" => Some(&sys::PsqlCompressor),
        "tsc" => Some(&js::TscCompressor),
        "eslint" => Some(&js::EslintCompressor),
        "go" => Some(&go::GoCompressor),
        "curl" => Some(&sys::CurlCompressor),
        "jest" | "vitest" => Some(&js::JestCompressor),
        "ruff" => Some(&python::RuffCompressor),
        "mypy" => Some(&python::MypyCompressor),
        "golangci-lint" | "golangci_lint" => Some(&go::GolangciLintCompressor),
        "make" | "gmake" => Some(&sys::MakeCompressor),
        _ => None,
    }
}

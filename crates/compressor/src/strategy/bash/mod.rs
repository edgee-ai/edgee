//! Bash command output compressors.
//!
//! Each shell command that can be compressed gets its own module
//! implementing the `BashCompressor` trait.

mod cargo;
mod curl;
mod diff;
mod docker;
mod env;
mod eslint;
mod find;
mod go;
mod grep;
mod ls;
mod npm;
mod psql;
mod pytest;
mod tree;
mod tsc;

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
        "ls" => Some(&ls::LsCompressor),
        "tree" => Some(&tree::TreeCompressor),
        "find" => Some(&find::FindCompressor),
        "grep" | "rg" => Some(&grep::GrepCompressor),
        "diff" | "git" => Some(&diff::DiffCompressor),
        "cargo" => Some(&cargo::CargoCompressor),
        "docker" => Some(&docker::DockerCompressor),
        "env" | "printenv" => Some(&env::EnvCompressor),
        "npm" | "pnpm" | "npx" => Some(&npm::NpmCompressor),
        "pytest" | "python" => Some(&pytest::PytestCompressor),
        "psql" => Some(&psql::PsqlCompressor),
        "tsc" => Some(&tsc::TscCompressor),
        "eslint" => Some(&eslint::EslintCompressor),
        "go" => Some(&go::GoCompressor),
        "curl" => Some(&curl::CurlCompressor),
        _ => None,
    }
}

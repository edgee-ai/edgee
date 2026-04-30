use anyhow::Result;

setup_command! {
    /// Emit a machine-readable JSON report instead of human-readable text.
    #[arg(long)]
    pub json: bool,
}

pub async fn run(_opts: Options) -> Result<()> {
    // Implementation lands in Part 2.
    Ok(())
}

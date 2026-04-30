use anyhow::Result;

setup_command! {
    /// Apply the fix without prompting for confirmation.
    #[arg(long)]
    pub yes: bool,
}

pub async fn run(_opts: Options) -> Result<()> {
    // Implementation lands in Part 2.
    Ok(())
}

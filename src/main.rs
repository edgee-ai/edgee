use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    edgee_cli::run().await
}

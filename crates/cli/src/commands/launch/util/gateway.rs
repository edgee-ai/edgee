pub async fn run_with_gateway(
    gateway: crate::local_gateway::LocalGatewayHandle,
    mut cmd: tokio::process::Command,
    not_found_msg: &'static str,
) -> anyhow::Result<()> {
    let mut gw_task = gateway.task;
    let gw_shutdown = gateway.shutdown_tx;

    let mut child = cmd.spawn().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            anyhow::anyhow!("{not_found_msg}")
        } else {
            anyhow::anyhow!(e)
        }
    })?;

    let status = tokio::select! {
        result = child.wait() => {
            let _ = gw_shutdown.send(());
            result?
        }
        result = &mut gw_task => {
            let _ = child.kill().await;
            match result {
                Ok(Ok(())) => eprintln!("error: local gateway stopped unexpectedly"),
                Ok(Err(e)) => eprintln!("error: local gateway crashed: {e:#}"),
                Err(e) => eprintln!("error: local gateway task panicked: {e}"),
            }
            std::process::exit(1);
        }
    };

    if let Some(code) = status.code() {
        std::process::exit(code);
    }
    Ok(())
}

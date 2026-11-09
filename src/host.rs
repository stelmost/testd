//! Headless host mode: runs the daemon without a GUI.

use anyhow::{Result, anyhow};

use testd_daemon::daemon::Daemon;

pub async fn run() -> Result<()> {
	let shutdown = testd_client::prepare_shutdown_signal()?;
	let daemon = Daemon::new().await.map_err(|e| anyhow!("{e}"))?;

	// Run engine until interrupted.
	tokio::select! {
		result = daemon.run() => result?,
		signal = shutdown => {
			signal?;
			eprintln!("\n[stop]   shutting down");
		}
	}

	daemon.shutdown().await.map_err(|error| anyhow!(error))?;

	// Daemon::drop clears registrations and the managed Caddyfile; the OS
	// resolver setup is one-time and intentionally left in place.
	drop(daemon);
	Ok(())
}

//! Headless host mode: runs the daemon without a GUI.

use std::future::Future;
use std::io;

use anyhow::{Result, anyhow};

use testd_daemon::daemon::Daemon;

pub async fn run() -> Result<()> {
	let shutdown = testd_client::prepare_shutdown_signal()?;
	let daemon = Daemon::new().await.map_err(|e| anyhow!("{e}"))?;
	let result = run_until_shutdown(daemon.run(), shutdown, daemon.shutdown()).await;

	// Daemon::drop clears registrations and the managed Caddyfile; the OS
	// resolver setup is one-time and intentionally left in place.
	drop(daemon);
	result
}

async fn run_until_shutdown(
	serving: impl Future<Output = io::Result<()>>,
	signal: impl Future<Output = io::Result<()>>,
	cleanup: impl Future<Output = Result<(), String>>,
) -> Result<()> {
	/* Preserve execution errors until owned provider cleanup has completed. */
	let result = tokio::select! {
		result = serving => result,
		signal = signal => {
			if signal.is_ok() {
				eprintln!("\n[stop]   shutting down");
			}
			signal
		}
	};
	match (result, cleanup.await) {
		(Ok(()), Ok(())) => Ok(()),
		(Err(error), Ok(())) => Err(error.into()),
		(Ok(()), Err(error)) => Err(anyhow!(error)),
		(Err(error), Err(cleanup)) => Err(anyhow!("{error}; cleanup: {cleanup}")),
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::cell::Cell;
	use std::future::{pending, ready};

	#[tokio::test]
	async fn headless_results_always_await_cleanup_and_preserve_errors() {
		const ENGINE_ERROR: &str = "fixture listener failure";
		const CLEANUP_ERROR: &str = "fixture provider cleanup failure";

		for execution_error in [None, Some(ENGINE_ERROR)] {
			for cleanup_error in [None, Some(CLEANUP_ERROR)] {
				let did_shutdown = Cell::new(false);
				let serving = ready(execution_error.map_or(Ok(()), |message| Err(io::Error::other(message))));
				let cleanup = async {
					did_shutdown.set(true);
					cleanup_error.map_or(Ok(()), |message| Err(message.to_owned()))
				};
				let result = run_until_shutdown(serving, pending(), cleanup).await;
				assert!(did_shutdown.get(), "an execution error skipped provider cleanup");
				assert_eq!(result.is_ok(), execution_error.is_none() && cleanup_error.is_none());
				if let Err(error) = result {
					let message = error.to_string();
					for expected in [execution_error, cleanup_error].into_iter().flatten() {
						assert!(message.contains(expected), "{message}");
					}
				}
			}
		}
	}

	#[tokio::test]
	async fn shutdown_signals_always_finish_cleanup() {
		const SIGNAL_ERROR: &str = "fixture shutdown signal failure";

		for signal_error in [None, Some(SIGNAL_ERROR)] {
			let did_shutdown = Cell::new(false);
			let signal = ready(signal_error.map_or(Ok(()), |message| Err(io::Error::other(message))));
			let cleanup = async {
				did_shutdown.set(true);
				Ok(())
			};
			let result = run_until_shutdown(pending(), signal, cleanup).await;
			assert!(did_shutdown.get(), "a shutdown signal error skipped provider cleanup");
			assert_eq!(result.is_ok(), signal_error.is_none());
			if let Some(expected) = signal_error {
				assert!(result.unwrap_err().to_string().contains(expected));
			}
		}
	}
}

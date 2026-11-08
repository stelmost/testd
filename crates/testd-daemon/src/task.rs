use tokio::task::JoinHandle;

/* Unlike a bare JoinHandle, daemon-owned work must stop when its owner disappears. */
pub(crate) struct BackgroundTask(pub(crate) JoinHandle<()>);

impl Drop for BackgroundTask {
	fn drop(&mut self) {
		self.0.abort();
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn dropping_a_task_releases_its_resources() {
		let (released, release) = tokio::sync::oneshot::channel::<()>();
		let task = BackgroundTask(tokio::spawn(async move {
			let _released = released;
			std::future::pending::<()>().await;
		}));
		drop(task);
		assert!(
			tokio::time::timeout(std::time::Duration::from_secs(1), release)
				.await
				.unwrap()
				.is_err()
		);
	}
}

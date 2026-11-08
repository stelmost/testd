//! Quick launches: saved commands with names owned for the command's lifetime.
//!
//! This is a local-only feature. The dashboard serves it from the loopback
//! listener; the authenticated listener never exposes process control.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot, watch};

use testd_client::Entry;
use testd_client::supervisor::{self, EntryState, LaunchEvent, LaunchSpec, OutputStream};
use testd_core::config::{config_dir, write_private};

const FILE: &str = "launches.json";
const MAX_LOG_LINES: usize = 500;

/// One name:target pair in a saved launch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchEntryDef {
	pub domain: String,
	/// Empty, `auto`, a port, or an explicit `host:port`.
	#[serde(default)]
	pub target: String,
}

/// A saved quick launch. Running state is deliberately separate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LaunchDefinition {
	#[serde(default)]
	pub id: String,
	pub label: String,
	pub program: String,
	#[serde(default)]
	pub args: Vec<String>,
	#[serde(default)]
	pub cwd: Option<String>,
	#[serde(default)]
	pub env: Vec<(String, String)>,
	pub entries: Vec<LaunchEntryDef>,
	#[serde(default)]
	pub share: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct LaunchView {
	#[serde(flatten)]
	pub definition: LaunchDefinition,
	pub running: bool,
	pub states: Vec<EntryStateView>,
	pub logs: Vec<LogLine>,
	pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct EntryStateView {
	pub domain: String,
	pub target: String,
	pub state: EntryState,
	pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LogLine {
	pub stream: OutputStream,
	pub line: String,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum RunPhase {
	Starting,
	Running,
	Stopping,
	#[default]
	Finished,
}

impl RunPhase {
	fn is_active(self) -> bool {
		self != Self::Finished
	}
}

#[derive(Default)]
struct RunState {
	phase: RunPhase,
	states: Vec<EntryStateView>,
	logs: VecDeque<LogLine>,
	error: Option<String>,
}

struct RebindRequest {
	domain: String,
	target: String,
	respond: oneshot::Sender<Result<(), String>>,
}

struct Running {
	stop: mpsc::Sender<()>,
	rebind: mpsc::Sender<RebindRequest>,
	state: Arc<Mutex<RunState>>,
	done: watch::Receiver<bool>,
}

/// Definitions plus the currently running supervisors.
pub struct LaunchBook {
	definitions: Mutex<Vec<LaunchDefinition>>,
	running: Mutex<HashMap<String, Running>>,
	path: PathBuf,
	/// Any change to definitions or live state, for live frontends.
	revision: watch::Sender<u64>,
}

impl LaunchBook {
	pub fn load() -> std::io::Result<Self> {
		Self::load_from(path())
	}

	pub(crate) fn load_from(path: PathBuf) -> std::io::Result<Self> {
		let definitions = match std::fs::read(&path) {
			Ok(data) => serde_json::from_slice::<Vec<LaunchDefinition>>(&data).map_err(|error| {
				std::io::Error::new(std::io::ErrorKind::InvalidData, format!("{}: {error}", path.display()))
			})?,
			Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
			Err(error) => return Err(error),
		};
		Ok(Self {
			definitions: Mutex::new(definitions),
			running: Mutex::new(HashMap::new()),
			path,
			revision: watch::channel(0).0,
		})
	}

	/// Any change to launch definitions or live state, for live frontends.
	pub fn state_changes(&self) -> watch::Receiver<u64> {
		self.revision.subscribe()
	}

	fn bump(&self) {
		self.revision.send_modify(|revision| *revision = revision.wrapping_add(1));
	}

	fn save(&self, definitions: &[LaunchDefinition]) -> Result<(), String> {
		let data = serde_json::to_vec_pretty(definitions).map_err(|e| e.to_string())?;
		let path = &self.path;
		if let Some(parent) = path.parent() {
			std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
		}
		write_private(path, &data).map_err(|e| e.to_string())
	}

	pub fn upsert(&self, mut definition: LaunchDefinition) -> Result<LaunchDefinition, String> {
		if definition.label.trim().is_empty() {
			return Err("a launch needs a label".into());
		}
		if definition.program.trim().is_empty() {
			return Err("a launch needs a program".into());
		}
		if definition.entries.is_empty() {
			return Err("a launch needs at least one name".into());
		}
		let mut domains = std::collections::HashSet::new();
		for entry in &definition.entries {
			if entry.domain.trim().is_empty() {
				return Err("every entry needs a name".into());
			}
			if !domains.insert(entry.domain.clone()) {
				return Err(format!("duplicate name: {}", entry.domain));
			}
		}
		if definition.id.is_empty() {
			definition.id = new_id();
		}

		let mut definitions = self.definitions.lock().expect("launch definitions poisoned");
		let mut replacement = definitions.clone();
		match replacement.iter_mut().find(|saved| saved.id == definition.id) {
			Some(saved) => *saved = definition.clone(),
			None => replacement.push(definition.clone()),
		}
		self.save(&replacement)?;
		*definitions = replacement;
		drop(definitions);
		self.bump();
		Ok(definition)
	}

	pub fn delete(&self, id: &str) -> Result<(), String> {
		let mut running = self.running.lock().expect("launch state poisoned");
		if running
			.get(id)
			.is_some_and(|run| run.state.lock().expect("launch state poisoned").phase.is_active())
		{
			return Err("stop the launch before deleting it".into());
		}
		let mut definitions = self.definitions.lock().expect("launch definitions poisoned");
		let replacement: Vec<_> = definitions.iter().filter(|saved| saved.id != id).cloned().collect();
		self.save(&replacement)?;
		*definitions = replacement;
		running.remove(id);
		drop(definitions);
		drop(running);
		self.bump();
		Ok(())
	}

	/// Every definition with its live state, output, and last error.
	pub fn list(&self) -> Vec<LaunchView> {
		let definitions = self.definitions.lock().expect("launch definitions poisoned").clone();
		let running = self.running.lock().expect("launch state poisoned");
		definitions
			.into_iter()
			.map(|definition| {
				let live = running.get(&definition.id).map(|state| {
					let state = state.state.lock().expect("launch state poisoned");
					(
						state.phase.is_active(),
						state.states.clone(),
						state.logs.iter().cloned().collect(),
						state.error.clone(),
					)
				});
				let (is_running, states, logs, error) = live.unwrap_or_default();
				LaunchView { definition, running: is_running, states, logs, error }
			})
			.collect()
	}

	/// Start a saved launch. The supervisor connects before spawning the child;
	/// that work happens off the request so the dashboard stays responsive.
	pub fn start(self: &Arc<Self>, id: &str) -> Result<(), String> {
		/* Hold the reservation lock through insertion so concurrent starts cannot spawn two supervisors. */
		let mut running = self.running.lock().expect("launch state poisoned");
		if running
			.get(id)
			.is_some_and(|run| run.state.lock().expect("launch state poisoned").phase.is_active())
		{
			return Err("launch is already running".into());
		}

		let definition = self
			.definitions
			.lock()
			.expect("launch definitions poisoned")
			.iter()
			.find(|saved| saved.id == id)
			.cloned()
			.ok_or_else(|| format!("no launch with id {id}"))?;

		let config = crate::control::client_config()?;
		let mut spec = LaunchSpec::new(
			PathBuf::from(&definition.program),
			definition
				.entries
				.iter()
				.map(|entry| Entry::new(entry.domain.clone(), entry.target.clone()))
				.collect(),
		);
		spec.args = definition.args.clone();
		spec.cwd = definition.cwd.clone().filter(|cwd| !cwd.is_empty()).map(PathBuf::from);
		spec.env = definition.env.clone();
		spec.share = definition.share;

		let (stop, stop_rx) = mpsc::channel(1);
		let (rebind, rebind_rx) = mpsc::channel(4);
		let state = Arc::new(Mutex::new(RunState { phase: RunPhase::Starting, ..RunState::default() }));
		let (done_tx, done) = watch::channel(false);
		running.insert(id.to_owned(), Running { stop, rebind, state: Arc::clone(&state), done });
		drop(running);
		self.bump();

		let book = Arc::clone(self);
		tokio::spawn(async move {
			run_launch(book, spec, config, stop_rx, rebind_rx, state).await;
			let _ = done_tx.send(true);
		});
		Ok(())
	}

	pub fn stop(&self, id: &str) -> Result<(), String> {
		let running = self.running.lock().expect("launch state poisoned");
		let Some(run) = running.get(id) else { return Ok(()) };
		let mut state = run.state.lock().expect("launch state poisoned");
		match state.phase {
			RunPhase::Starting | RunPhase::Running => {
				run.stop.try_send(()).map_err(|_| "launch is already stopping".to_owned())?;
				state.phase = RunPhase::Stopping;
			},
			RunPhase::Stopping | RunPhase::Finished => return Err("launch is already stopping".into()),
		}
		drop(state);
		self.bump();
		Ok(())
	}

	pub async fn rebind(&self, id: &str, domain: String, target: String) -> Result<(), String> {
		let (respond, response) = oneshot::channel();
		let sender = {
			let running = self.running.lock().expect("launch state poisoned");
			match running.get(id) {
				Some(run) => match run.state.lock().expect("launch state poisoned").phase {
					RunPhase::Starting => return Err("launch is still starting".into()),
					RunPhase::Running => Some(run.rebind.clone()),
					RunPhase::Stopping | RunPhase::Finished => return Err("launch stopped".into()),
				},
				None => None,
			}
		};
		let Some(sender) = sender else { return Err("launch is not running".into()) };
		sender
			.send(RebindRequest { domain, target, respond })
			.await
			.map_err(|_| "launch stopped".to_owned())?;
		response.await.map_err(|_| "launch stopped".to_owned())?
	}

	/// Stop every running launch. Called when the daemon shuts down.
	pub async fn shutdown(&self) {
		let running: Vec<(mpsc::Sender<()>, watch::Receiver<bool>)> = self
			.running
			.lock()
			.expect("launch state poisoned")
			.values()
			.map(|state| (state.stop.clone(), state.done.clone()))
			.collect();
		for (stop, done) in &running {
			if !*done.borrow() {
				let _ = stop.send(()).await;
			}
		}
		for (_, done) in running {
			let mut done = done;
			if *done.borrow() {
				continue;
			}
			let _ = tokio::time::timeout(Duration::from_secs(5), done.changed()).await;
		}
	}
}

fn path() -> PathBuf {
	config_dir().join(FILE)
}

fn new_id() -> String {
	static NEXT: AtomicU64 = AtomicU64::new(1);
	let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_nanos();
	format!("{now:x}-{:x}", NEXT.fetch_add(1, Ordering::SeqCst))
}

async fn run_launch(
	book: Arc<LaunchBook>,
	spec: LaunchSpec,
	config: testd_core::config::ClientConfig,
	mut stop: mpsc::Receiver<()>,
	mut rebind: mpsc::Receiver<RebindRequest>,
	state: Arc<Mutex<RunState>>,
) {
	let mut launch = match supervisor::launch(spec, config).await {
		Ok(launch) => launch,
		Err(e) => {
			let mut state = state.lock().expect("launch state poisoned");
			state.phase = RunPhase::Finished;
			state.error = Some(format!("{e:#}"));
			drop(state);
			book.bump();
			return;
		},
	};
	let Some(mut events) = launch.take_events() else {
		state.lock().expect("launch state poisoned").phase = RunPhase::Finished;
		book.bump();
		return;
	};

	let mut rebinding: Option<Pin<Box<dyn Future<Output = ()> + Send>>> = None;
	loop {
		tokio::select! {
			_ = async {
				match rebinding.as_mut() {
					Some(request) => request.await,
					None => std::future::pending().await,
				}
			} => rebinding = None,
			event = events.recv() => {
				let Some(event) = event else { break };
				record(&state, &event);
				let changed = matches!(&event, LaunchEvent::State { .. } | LaunchEvent::Exited { .. } | LaunchEvent::Failed { .. });
				if changed {
					book.bump();
				}
				let done = matches!(&event, LaunchEvent::Exited { .. } | LaunchEvent::Failed { .. });
				if done {
					break;
				}
			},
			Some(()) = stop.recv() => {
				drop(rebinding.take());
				state.lock().expect("launch state poisoned").phase = RunPhase::Stopping;
				let _ = launch.stop().await;
				while let Ok(event) = events.try_recv() {
					record(&state, &event);
				}
				book.bump();
				break;
			},
			Some(request) = rebind.recv() => {
				if rebinding.is_some() {
					let _ = request.respond.send(Err("launch is already rebinding".into()));
				} else {
					let launch = &launch;
					rebinding = Some(Box::pin(async move {
						let result = launch.rebind(&request.domain, &request.target).await;
						let _ = request.respond.send(result);
					}));
				}
			},
		}
	}
	book.finish(&state);
}

impl LaunchBook {
	fn finish(&self, state: &Mutex<RunState>) {
		/* Finish this run, not a replacement that may now have the same definition ID. */
		state.lock().expect("launch state poisoned").phase = RunPhase::Finished;
		self.bump();
	}
}

fn record(state: &Arc<Mutex<RunState>>, event: &LaunchEvent) {
	let mut state = state.lock().expect("launch state poisoned");
	match event {
		LaunchEvent::State { domain, target, state: entry, error } => {
			if let Some(existing) = state.states.iter_mut().find(|item| item.domain == *domain) {
				existing.target = target.clone();
				existing.state = *entry;
				if error.is_some() || *entry != EntryState::Configured {
					existing.error = error.clone();
				}
			} else {
				state.states.push(EntryStateView {
					domain: domain.clone(),
					target: target.clone(),
					state: *entry,
					error: error.clone(),
				});
			}
		},
		LaunchEvent::Output { stream, line } => {
			if state.logs.len() == MAX_LOG_LINES {
				state.logs.pop_front();
			}
			state.logs.push_back(LogLine { stream: *stream, line: line.clone() });
		},
		LaunchEvent::Failed { message } => {
			state.phase = RunPhase::Finished;
			state.error = Some(message.clone());
		},
		LaunchEvent::Exited { code } => {
			state.phase = RunPhase::Finished;
			if state.error.is_none() {
				state.error = match code {
					Some(code) if *code != 0 => Some(format!("command exited with status {code}")),
					_ => state.states.iter().find_map(|entry| entry.error.clone()),
				};
			}
		},
		LaunchEvent::Started { .. } => {
			if state.phase == RunPhase::Starting {
				state.phase = RunPhase::Running;
			}
		},
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn definition(id: &str) -> LaunchDefinition {
		LaunchDefinition {
			id: id.into(),
			label: "app".into(),
			program: "/bin/true".into(),
			args: Vec::new(),
			cwd: None,
			env: Vec::new(),
			entries: vec![LaunchEntryDef { domain: "app.example".into(), target: "3000".into() }],
			share: false,
		}
	}

	fn book(path: PathBuf) -> LaunchBook {
		LaunchBook {
			definitions: Mutex::new(Vec::new()),
			running: Mutex::new(HashMap::new()),
			path,
			revision: watch::channel(0).0,
		}
	}

	#[test]
	fn definitions_round_trip_and_reject_duplicates() {
		let dir = tempfile::tempdir().unwrap();
		let book = book(dir.path().join("launches.json"));
		let saved = book.upsert(definition("one")).unwrap();
		assert_eq!(saved.id, "one");
		assert_eq!(book.list().len(), 1);

		let mut duplicate = definition("one");
		duplicate.entries.push(duplicate.entries[0].clone());
		assert!(book.upsert(duplicate).is_err());
	}

	#[test]
	fn failed_saves_and_deletes_keep_live_definitions() {
		let dir = tempfile::tempdir().unwrap();
		let book = book(dir.path().join("missing").join("launches.json"));
		book.definitions.lock().unwrap().push(definition("one"));
		std::fs::write(dir.path().join("missing"), b"not a directory").unwrap();
		let mut replacement = definition("one");
		replacement.label = "unsaved".into();
		assert!(book.upsert(replacement).is_err());
		assert_eq!(book.list()[0].definition.label, "app");
		assert!(book.delete("one").is_err());
		assert_eq!(book.list().len(), 1);
	}

	#[test]
	fn corrupt_launch_files_are_not_replaced_with_empty_books() {
		let dir = tempfile::tempdir().unwrap();
		let path = dir.path().join("launches.json");
		std::fs::write(&path, b"invalid JSON").unwrap();
		assert!(LaunchBook::load_from(path.clone()).is_err());
		assert_eq!(std::fs::read(path).unwrap(), b"invalid JSON");
	}

	#[test]
	fn concurrent_saves_keep_every_definition() {
		let dir = tempfile::tempdir().unwrap();
		let book = Arc::new(book(dir.path().join("launches.json")));
		std::thread::scope(|scope| {
			for index in 0..16 {
				let book = Arc::clone(&book);
				scope.spawn(move || book.upsert(definition(&index.to_string())).unwrap());
			}
		});
		assert_eq!(book.list().len(), 16);
		assert_eq!(LaunchBook::load_from(book.path.clone()).unwrap().list().len(), 16);
	}

	fn reserve_run(book: &LaunchBook, phase: RunPhase) -> (Arc<Mutex<RunState>>, mpsc::Receiver<()>) {
		let state = Arc::new(Mutex::new(RunState { phase, ..RunState::default() }));
		let (stop, receiver) = mpsc::channel(1);
		let (rebind, _) = mpsc::channel(4);
		let (_, done) = watch::channel(false);
		book.running
			.lock()
			.unwrap()
			.insert("one".into(), Running { stop, rebind, state: Arc::clone(&state), done });
		(state, receiver)
	}

	#[test]
	fn launch_phases_keep_the_existing_running_view() {
		let dir = tempfile::tempdir().unwrap();
		let book = Arc::new(book(dir.path().join("launches.json")));
		book.upsert(definition("one")).unwrap();
		let (state, mut stop) = reserve_run(&book, RunPhase::Starting);
		assert!(book.list()[0].running);

		record(&state, &LaunchEvent::Started { pid: 1 });
		assert_eq!(state.lock().unwrap().phase, RunPhase::Running);
		book.stop("one").unwrap();
		assert_eq!(state.lock().unwrap().phase, RunPhase::Stopping);
		stop.try_recv().unwrap();
		assert!(book.stop("one").is_err(), "draining the command must not permit another stop");
		assert!(book.start("one").is_err());
		assert!(book.delete("one").is_err());
		assert!(book.list()[0].running, "cleanup still owns the launch");

		record(&state, &LaunchEvent::Output { stream: OutputStream::Stderr, line: "last line".into() });
		record(&state, &LaunchEvent::Exited { code: Some(0) });
		let view = book.list().remove(0);
		assert!(!view.running);
		assert_eq!(view.logs[0].line, "last line");
		let json = serde_json::to_value(view).unwrap();
		assert_eq!(json["running"], false);
		assert!(json.get("phase").is_none(), "the public format remains unchanged");
		book.delete("one").unwrap();
	}

	#[test]
	fn late_started_events_do_not_undo_stopping_or_completion() {
		for phase in [RunPhase::Stopping, RunPhase::Finished] {
			let state = Arc::new(Mutex::new(RunState { phase, ..RunState::default() }));
			record(&state, &LaunchEvent::Started { pid: 1 });
			assert_eq!(state.lock().unwrap().phase, phase);
		}
	}

	#[test]
	fn completing_an_old_run_does_not_finish_its_replacement() {
		let dir = tempfile::tempdir().unwrap();
		let book = book(dir.path().join("launches.json"));
		let (old, _) = reserve_run(&book, RunPhase::Finished);
		let (replacement, _) = reserve_run(&book, RunPhase::Starting);
		book.finish(&old);
		assert_eq!(replacement.lock().unwrap().phase, RunPhase::Starting);
	}

	#[tokio::test]
	async fn rebind_is_only_accepted_while_running() {
		let dir = tempfile::tempdir().unwrap();
		let book = book(dir.path().join("launches.json"));
		for phase in [RunPhase::Starting, RunPhase::Stopping, RunPhase::Finished] {
			let (_state, _stop) = reserve_run(&book, phase);
			assert!(book.rebind("one", "app.example".into(), "127.0.0.1:3001".into()).await.is_err());
		}
	}

	#[tokio::test]
	async fn a_running_launch_forwards_rebind_and_returns_its_result() {
		let dir = tempfile::tempdir().unwrap();
		let book = book(dir.path().join("launches.json"));
		let (_state, _stop) = reserve_run(&book, RunPhase::Running);
		let (sender, mut requests) = mpsc::channel(1);
		book.running.lock().unwrap().get_mut("one").unwrap().rebind = sender;
		let (result, ()) = tokio::time::timeout(Duration::from_secs(3), async {
			tokio::join!(book.rebind("one", "app.example".into(), "127.0.0.1:3001".into()), async {
				let request = requests.recv().await.unwrap();
				assert_eq!(request.domain, "app.example");
				assert_eq!(request.target, "127.0.0.1:3001");
				request.respond.send(Err("probe failed".into())).unwrap();
			})
		})
		.await
		.expect("rebind must receive a response");
		assert_eq!(result, Err("probe failed".into()));
	}

	#[test]
	fn a_failed_launch_retains_its_error_after_completion() {
		let state = Arc::new(Mutex::new(RunState { phase: RunPhase::Starting, ..RunState::default() }));
		record(&state, &LaunchEvent::Failed { message: "host unavailable".into() });
		record(&state, &LaunchEvent::Exited { code: None });
		let state = state.lock().unwrap();
		assert_eq!(state.phase, RunPhase::Finished);
		assert_eq!(state.error.as_deref(), Some("host unavailable"));
	}

	#[test]
	fn failed_probes_and_command_exits_remain_visible() {
		let state = Arc::new(Mutex::new(RunState::default()));
		let event = |error| LaunchEvent::State {
			domain: "app.example".into(),
			target: "127.0.0.1:3000".into(),
			state: EntryState::Configured,
			error,
		};
		record(&state, &event(Some("name is already registered".into())));
		record(&state, &event(None));
		record(&state, &LaunchEvent::Exited { code: None });
		assert_eq!(state.lock().unwrap().error.as_deref(), Some("name is already registered"));
		assert!(state.lock().unwrap().states[0].error.is_some());

		let state = Arc::new(Mutex::new(RunState::default()));
		record(&state, &LaunchEvent::Exited { code: Some(7) });
		assert_eq!(state.lock().unwrap().error.as_deref(), Some("command exited with status 7"));
	}

	#[test]
	fn omitting_the_id_assigns_one() {
		let dir = tempfile::tempdir().unwrap();
		let book = book(dir.path().join("launches.json"));
		let saved = book.upsert(definition("")).unwrap();
		assert!(!saved.id.is_empty());
	}
}

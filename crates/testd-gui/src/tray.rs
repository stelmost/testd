/*! Native tray presentation. Every value in Tray stays on the event-loop thread. */

use std::cell::RefCell;
use std::io::Cursor;

use tray_icon::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};

use testd_core::Registration;
use testd_daemon::health::Health;
use testd_daemon::launch::LaunchView;
use testd_daemon::share::ShareView;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Phase {
	Starting,
	Setup,
	Running,
	Failed,
	Stopping,
}

#[derive(Clone, PartialEq)]
pub struct State {
	pub phase: Phase,
	pub regs: Vec<Registration>,
	pub health: Health,
	pub autostart: Option<bool>,
	pub shares: Vec<ShareView>,
	pub launches: Vec<LaunchRow>,
	pub tld: String,
	pub has_request: bool,
}

impl State {
	pub fn new(phase: Phase) -> Self {
		Self {
			phase,
			regs: Vec::new(),
			health: Health::default(),
			autostart: None,
			shares: Vec::new(),
			launches: Vec::new(),
			tld: testd_core::config::host_tld().into(),
			has_request: false,
		}
	}

	fn same_menu(&self, other: &Self) -> bool {
		self.phase == other.phase
			&& self.regs == other.regs
			&& self.health == other.health
			&& self.shares == other.shares
			&& self.launches == other.launches
			&& self.tld == other.tld
	}

	fn icon(&self) -> IconState {
		IconState::current(self.phase == Phase::Running && self.health.healthy(), self.has_request)
	}
}

#[derive(Clone, PartialEq, Eq)]
pub struct LaunchRow {
	id: String,
	label: String,
	running: bool,
	is_ssh: bool,
}

/* Native callbacks are Send + Sync; only the clicked value crosses that boundary, never the main-thread widget. */
thread_local! {
	static LOGIN_ITEM: RefCell<Option<CheckMenuItem>> = const { RefCell::new(None) };
}

fn publish_login_item(item: Option<CheckMenuItem>) {
	LOGIN_ITEM.with(|current| current.replace(item));
}

pub fn clicked_login_preference() -> Option<bool> {
	LOGIN_ITEM.with(|current| {
		current
			.borrow()
			.as_ref()
			.filter(|item| item.is_enabled())
			.map(CheckMenuItem::is_checked)
	})
}

pub struct Tray {
	icon: TrayIcon,
	state: State,
	autostart: Option<CheckMenuItem>,
}

impl Tray {
	pub fn new() -> anyhow::Result<Self> {
		let state = State::new(Phase::Starting);
		let (menu, autostart) = build_menu(&state)?;
		let icon = TrayIconBuilder::new()
			.with_tooltip("testd")
			.with_icon(tray_icon(state.icon())?)
			.with_icon_as_template(state.icon().as_template())
			.with_menu(Box::new(menu))
			.with_menu_on_left_click(true)
			.build()?;
		publish_login_item(autostart.clone());
		Ok(Self { icon, state, autostart })
	}

	pub fn update(&mut self, next: State) -> anyhow::Result<()> {
		if !self.state.same_menu(&next) {
			let (menu, autostart) = build_menu(&next)?;
			self.icon.set_menu(Some(Box::new(menu)));
			self.autostart = autostart;
			publish_login_item(self.autostart.clone());
		}
		if let Some(item) = &self.autostart {
			item.set_enabled(next.autostart.is_some());
			item.set_text(login_label(next.autostart));
			if let Some(enabled) = next.autostart {
				item.set_checked(enabled);
			}
		}
		let next_icon = next.icon();
		if next_icon != self.state.icon() {
			let icon = Some(tray_icon(next_icon)?);
			/* The template setter only updates the icon on macOS. */
			#[cfg(target_os = "macos")]
			self.icon.set_icon_with_as_template(icon, next_icon.as_template())?;
			#[cfg(not(target_os = "macos"))]
			self.icon.set_icon(icon)?;
		}
		self.state = next;
		Ok(())
	}
}

impl Drop for Tray {
	fn drop(&mut self) {
		publish_login_item(None);
	}
}

fn login_label(state: Option<bool>) -> &'static str {
	if state.is_some() { "Launch at login" } else { "Launch at login (unavailable)" }
}

fn build_menu(state: &State) -> anyhow::Result<(Menu, Option<CheckMenuItem>)> {
	let menu = Menu::new();
	if state.phase != Phase::Running {
		let label = match state.phase {
			Phase::Setup => "Setup is not complete",
			Phase::Starting => "Starting testd...",
			Phase::Failed => "testd could not start",
			Phase::Stopping => "Stopping testd...",
			Phase::Running => unreachable!(),
		};
		menu.append(&MenuItem::with_id("status", label, false, None))?;
		if state.phase == Phase::Setup {
			menu.append(&MenuItem::with_id("dashboard-open", "Continue setup", true, None))?;
		}
		menu.append(&MenuItem::with_id("quit", "Exit", state.phase != Phase::Stopping, None))?;
		return Ok((menu, None));
	}
	append_domains(&menu, state)?;
	append_launches(&menu, &state.launches)?;
	append_shares(&menu, &state.shares)?;
	let autostart = CheckMenuItem::with_id(
		"autostart",
		login_label(state.autostart),
		state.autostart.is_some(),
		state.autostart.unwrap_or(false),
		None,
	);
	menu.append(&autostart)?;
	menu.append(&MenuItem::with_id("quit", "Exit", true, None))?;
	Ok((menu, Some(autostart)))
}

fn append_domains(menu: &Menu, state: &State) -> anyhow::Result<()> {
	if !state.health.resolver {
		menu.append(&MenuItem::with_id("setup-dns", "Set up DNS", true, None))?;
	}
	if !state.health.root_trusted {
		menu.append(&MenuItem::with_id("trust-root", "Trust certificate", true, None))?;
	}
	if !state.health.healthy() {
		menu.append(&PredefinedMenuItem::separator())?;
	}
	if state.regs.is_empty() {
		menu.append(&MenuItem::with_id("no-domains", "No active domains", false, None))?;
	}
	for registration in &state.regs {
		let domain = &registration.domain;
		menu.append(&MenuItem::with_id(format!("domain-open:{domain}"), domain, true, None))?;
	}
	menu.append(&MenuItem::with_id("dashboard-open", format!("testd.{} → dashboard", state.tld), true, None))?;
	menu.append(&PredefinedMenuItem::separator())?;
	Ok(())
}

fn append_launches(menu: &Menu, launches: &[LaunchRow]) -> anyhow::Result<()> {
	for (is_ssh, id, title) in [(false, "quick-launches", "Quick launches"), (true, "ssh-apps", "SSH apps")] {
		let mut entries = launches.iter().filter(|launch| launch.is_ssh == is_ssh).peekable();
		if entries.peek().is_none() {
			continue;
		}
		menu.append(&MenuItem::with_id(id, title, false, None))?;
		for launch in entries {
			menu.append(&MenuItem::with_id(format!("launch:{}", launch.id), &launch.label, false, None))?;
			let (action, label) = if launch.running { ("stop", "    Stop") } else { ("start", "    Start") };
			menu.append(&MenuItem::with_id(format!("launch-{action}:{}", launch.id), label, true, None))?;
		}
		menu.append(&PredefinedMenuItem::separator())?;
	}
	Ok(())
}

fn append_shares(menu: &Menu, shares: &[ShareView]) -> anyhow::Result<()> {
	if shares.is_empty() {
		return Ok(());
	}
	for share in shares {
		menu.append(&MenuItem::with_id(
			format!("share-open:{}", share.name),
			format!("Shared: {} → {} ({})", share.name, share.url, share.exposure.label()),
			true,
			None,
		))?;
		menu.append(&MenuItem::with_id(
			format!("share-stop:{}", share.name),
			format!("Stop sharing {}", share.name),
			true,
			None,
		))?;
	}
	menu.append(&PredefinedMenuItem::separator())?;
	Ok(())
}

pub fn summarize_launches(views: &[LaunchView]) -> Vec<LaunchRow> {
	views
		.iter()
		.map(|view| LaunchRow {
			id: view.definition.id.clone(),
			label: launch_label(view),
			running: view.running,
			is_ssh: view.definition.ssh.is_some(),
		})
		.collect()
}

fn launch_label(view: &LaunchView) -> String {
	if let Some(ssh) = &view.definition.ssh {
		return format!("{} ({} port {})", view.definition.label, ssh.host, ssh.port);
	}
	let mut ports = Vec::new();
	for entry in &view.definition.entries {
		let target = view
			.states
			.iter()
			.find(|state| state.domain == entry.domain)
			.map(|state| state.target.as_str())
			.unwrap_or(entry.target.as_str());
		if let Some(port) = target_port(target)
			&& !ports.contains(&port)
		{
			ports.push(port);
		}
	}
	if ports.is_empty() {
		view.definition.label.clone()
	} else {
		format!("{} :{}", view.definition.label, ports.join(", :"))
	}
}

fn target_port(target: &str) -> Option<&str> {
	let target = target.trim();
	if target.is_empty() || target.eq_ignore_ascii_case("auto") {
		return None;
	}
	let port = match target.strip_prefix('[') {
		Some(rest) => rest.split_once("]:").map(|(_, port)| port)?,
		None => target.rsplit_once(':').map_or(target, |(_, port)| port),
	};
	(!port.is_empty() && port.chars().all(|character| character.is_ascii_digit())).then_some(port)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IconState {
	Healthy,
	Request,
	Alert,
}

impl IconState {
	fn current(healthy: bool, has_request: bool) -> Self {
		if !healthy {
			Self::Alert
		} else if has_request {
			Self::Request
		} else {
			Self::Healthy
		}
	}

	fn as_template(self) -> bool {
		self == Self::Healthy
	}
}

fn tray_icon(state: IconState) -> anyhow::Result<Icon> {
	#[cfg(target_os = "macos")]
	let bytes: &'static [u8] = match state {
		IconState::Healthy => include_bytes!("../icons/tray-on-light.png"),
		IconState::Request => include_bytes!("../icons/tray-on-light-request.png"),
		IconState::Alert => include_bytes!("../icons/tray-on-light-alert.png"),
	};
	#[cfg(not(target_os = "macos"))]
	let bytes: &'static [u8] = match state {
		IconState::Healthy => include_bytes!("../icons/tray-plate.png"),
		IconState::Request => include_bytes!("../icons/tray-plate-request.png"),
		IconState::Alert => include_bytes!("../icons/tray-plate-alert.png"),
	};
	let mut decoder = png::Decoder::new(Cursor::new(bytes));
	decoder.set_transformations(png::Transformations::normalize_to_color8());
	let mut reader = decoder.read_info()?;
	let mut pixels = vec![
		0;
		reader
			.output_buffer_size()
			.ok_or_else(|| anyhow::anyhow!("tray icon is too large"))?
	];
	let info = reader.next_frame(&mut pixels)?;
	anyhow::ensure!(info.color_type == png::ColorType::Rgba, "embedded tray icons must be RGBA");
	pixels.truncate(info.buffer_size());
	Ok(Icon::from_rgba(pixels, info.width, info.height)?)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn alerts_outrank_requests_and_only_healthy_icons_are_templates() {
		assert_eq!(IconState::current(false, true), IconState::Alert);
		assert_eq!(IconState::current(true, true), IconState::Request);
		assert_eq!(IconState::current(true, false), IconState::Healthy);
		for state in [IconState::Healthy, IconState::Request, IconState::Alert] {
			assert!(tray_icon(state).is_ok());
			assert_eq!(state.as_template(), state == IconState::Healthy);
		}
	}

	#[test]
	fn launch_ports_keep_ipv6_and_auto_detection_semantics() {
		for (target, expected) in [
			("auto", None),
			("", None),
			("[::1]:3000", Some("3000")),
			("localhost:8080", Some("8080")),
			("3000", Some("3000")),
			("host:not-a-port", None),
		] {
			assert_eq!(target_port(target), expected);
		}
	}

	#[test]
	fn request_and_autostart_changes_do_not_replace_an_open_menu() {
		let current = State::new(Phase::Running);
		let mut next = current.clone();
		next.has_request = true;
		next.autostart = Some(true);
		assert!(current.same_menu(&next));
		next.health.resolver = true;
		assert!(!current.same_menu(&next));
	}
}

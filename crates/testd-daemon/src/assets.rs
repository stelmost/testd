//! Static files served by the dashboard and the setup server.
//!
//! Release builds embed them; development reads from the crate directory so an
//! edit does not need a rebuild.

pub const DASHBOARD_HTML: &str = "dashboard.html";
pub const DASHBOARD_CSS: &str = "dashboard.css";
pub const SETUP_HTML: &str = "setup.html";
const BRAND_MARK: &str = "../../assets/brand/testd-slim.svg";

/// Load one of the known assets by name.
pub fn load(name: &str) -> String {
	#[cfg(debug_assertions)]
	{
		let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(name);
		std::fs::read_to_string(&path).unwrap_or_else(|e| format!("failed to load {}: {e}", path.display()))
	}
	#[cfg(not(debug_assertions))]
	{
		match name {
			DASHBOARD_HTML => include_str!("../dashboard.html"),
			DASHBOARD_CSS => include_str!("../dashboard.css"),
			SETUP_HTML => include_str!("../setup.html"),
			BRAND_MARK => include_str!("../../../assets/brand/testd-slim.svg"),
			_ => "",
		}
		.to_owned()
	}
}

pub fn load_page(name: &str) -> String {
	load(name).replace("{brand_mark}", load(BRAND_MARK).trim_end())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn both_pages_include_the_slim_brand_mark() {
		let mark = include_str!("../../../assets/brand/testd-slim.svg").trim_end();
		for name in [DASHBOARD_HTML, SETUP_HTML] {
			let page = load_page(name);
			assert!(page.contains(mark), "{name} does not contain the slim brand mark");
		}
	}
}

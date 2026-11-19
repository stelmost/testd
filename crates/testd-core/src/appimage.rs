/*! Restore host values for processes launched from an AppImage's bundled runtime. */

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};

/// Changes to apply to a child launch environment before explicit user overrides.
pub fn host_environment() -> Vec<(OsString, Option<OsString>)> {
	host_environment_from(std::env::vars_os().collect())
}

fn host_environment_from(environment: BTreeMap<OsString, OsString>) -> Vec<(OsString, Option<OsString>)> {
	if !environment.contains_key(OsStr::new("APPIMAGE"))
		|| environment.get(OsStr::new("TESTD_APPIMAGE_HOST_ENV")).map(OsString::as_os_str) != Some(OsStr::new("1"))
	{
		return Vec::new();
	}
	let mut changes = Vec::new();
	for (key, value) in environment {
		let Some(name) = key.to_str() else { continue };
		if let Some(variable) = name.strip_prefix("TESTD_APPIMAGE_ORIGINAL_") {
			changes.push((OsString::from(variable), Some(value)));
		} else if let Some(variable) = name.strip_prefix("TESTD_APPIMAGE_UNSET_") {
			changes.push((OsString::from(variable), None));
		} else {
			continue;
		}
		changes.push((key, None));
	}
	for key in ["APPIMAGE", "APPDIR", "TESTD_APPIMAGE_HOST_ENV"] {
		changes.push((key.into(), None));
	}
	changes
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn restores_host_values_and_absence_without_leaking_bundle_metadata() {
		let mut environment: BTreeMap<OsString, OsString> = [
			("APPIMAGE", "/a space/Testd.AppImage"),
			("APPDIR", "/tmp/mounted"),
			("TESTD_APPIMAGE_HOST_ENV", "1"),
			("PYTHONHOME", "/tmp/mounted/usr"),
			("TESTD_APPIMAGE_UNSET_PYTHONHOME", "1"),
			("PATH", "/tmp/mounted/usr/bin:/usr/bin"),
			("TESTD_APPIMAGE_ORIGINAL_PATH", "/host/π:/usr/bin"),
			("TESTD_APPIMAGE_ORIGINAL_LD_LIBRARY_PATH", ""),
			("LD_LIBRARY_PATH", "/tmp/mounted/usr/lib"),
			("USER_SETTING", "retained"),
		]
		.into_iter()
		.map(|(key, value)| (key.into(), value.into()))
		.collect();
		for (key, value) in host_environment_from(environment.clone()) {
			match value {
				Some(value) => {
					environment.insert(key, value);
				},
				None => {
					environment.remove(&key);
				},
			}
		}
		assert_eq!(environment.get(OsStr::new("PATH")).unwrap(), "/host/π:/usr/bin");
		assert_eq!(environment.get(OsStr::new("LD_LIBRARY_PATH")).unwrap(), "");
		assert_eq!(environment.get(OsStr::new("USER_SETTING")).unwrap(), "retained");
		assert!(!environment.contains_key(OsStr::new("PYTHONHOME")));
		assert!(
			!environment
				.keys()
				.any(|key| key.to_string_lossy().starts_with("TESTD_APPIMAGE_"))
		);
		assert!(!environment.contains_key(OsStr::new("APPIMAGE")));
		assert!(!environment.contains_key(OsStr::new("APPDIR")));
	}

	#[test]
	fn leaves_unbundled_processes_unchanged() {
		for (key, value) in [("APPIMAGE", "/foreign/image"), ("TESTD_APPIMAGE_HOST_ENV", "1")] {
			assert!(host_environment_from([(key.into(), value.into())].into()).is_empty());
		}
	}
}

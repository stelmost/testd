//! Own only the exact certificate and admin trust entry created by this installation.

use std::io::{self, Write};
use std::path::PathBuf;

use super::{Elevation, macos, ownership, sha1_hex, sha256_hex, shell_quote, silent_command};

#[cfg(test)]
#[path = "macos_trust_tests.rs"]
mod tests;

const KEYCHAIN: &str = "/Library/Keychains/System.keychain";

struct Root {
	pem: String,
	sha1: String,
	marker: PathBuf,
}

impl Root {
	fn new(pem: &str) -> io::Result<Self> {
		let der = super::single_anchor_der(pem)?;
		x509_parser::parse_x509_certificate(&der)
			.map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error.to_string()))?;
		Ok(Self {
			pem: pem.to_owned(),
			sha1: sha1_hex(&der).to_uppercase(),
			marker: PathBuf::from(format!("/Library/Keychains/.testd-root-{}.owner", sha256_hex(&der))),
		})
	}

	fn script(&self, owner: &str, snapshot: &str) -> String {
		let helpers = macos::file_script(&self.marker, owner);
		/* A trust receipt occupies the path itself; the file helper supplies validation functions. */
		format!(
			"{helpers}\nmarker=$path\ndirectory='/Library/Keychains'\nkeychain={keychain}\n\
			certificate={certificate}\nsha1={sha1}\nsnapshot={snapshot}\n{trust}",
			keychain = shell_quote(KEYCHAIN),
			certificate = shell_quote(&self.pem),
			sha1 = shell_quote(&self.sha1),
			snapshot = shell_quote(snapshot),
			trust = include_str!("macos-trust.sh"),
		)
	}
}

fn certificates() -> io::Result<String> {
	let output = silent_command("security")
		.args(["find-certificate", "-a", "-p", KEYCHAIN])
		.output()?;
	if !output.status.success() {
		return Err(io::Error::other("could not inspect the system keychain; no trust change was attempted"));
	}
	String::from_utf8(output.stdout).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

fn matching_count(root: &Root, store: &str) -> io::Result<usize> {
	let expected = super::single_anchor_der(&root.pem)?;
	let mut count = 0;
	for certificate in super::parse_macos_roots(store) {
		if certificate.hash.eq_ignore_ascii_case(&root.sha1) {
			if super::pem_to_der(&certificate.pem)? != expected {
				return Err(io::Error::other("certificate SHA-1 collision; system trust was left untouched"));
			}
			count += 1;
		}
	}
	if count > 1 {
		return Err(io::Error::other("duplicate certificate identity; system trust was left untouched"));
	}
	Ok(count)
}

pub(super) fn is_trusted(pem: &str) -> bool {
	let Ok(root) = Root::new(pem) else { return false };
	let Ok(store) = certificates() else { return false };
	if matching_count(&root, &store).ok() != Some(1) {
		return false;
	}
	let Ok(mut certificate) = testd_core::private_file::temporary_in(&std::env::temp_dir()) else { return false };
	if certificate.write_all(pem.as_bytes()).is_err() {
		return false;
	}
	silent_command("security")
		.args(["verify-cert", "-L", "-p", "basic", "-c"])
		.arg(certificate.path())
		.output()
		.is_ok_and(|output| output.status.success())
}

pub(super) fn install(pem: &str, elevation: Elevation) -> io::Result<()> {
	let root = Root::new(pem)?;
	let store = certificates()?;
	let certificate_count = matching_count(&root, &store)?;
	if certificate_count == 1 && is_trusted(pem) {
		return Ok(());
	}
	let owner = ownership::InstallationId::load_or_create()?;
	let script = root.script(&owner.fingerprint(), &sha256_hex(store.as_bytes()));
	macos::run(&format!("{script}\ncertificate_count={certificate_count}\ninstall_trust"), elevation)?;
	if !is_trusted(pem) {
		return Err(io::Error::other("the installed certificate did not pass the macOS trust check"));
	}
	Ok(())
}

pub(super) fn remove(pem: &str, elevation: Elevation) -> io::Result<()> {
	let root = Root::new(pem)?;
	let store = certificates()?;
	let certificate_count = matching_count(&root, &store)?;
	if certificate_count == 0 && !macos::present(&root.marker)? {
		return Ok(());
	}
	let owner = ownership::InstallationId::load()?.ok_or_else(|| {
		io::Error::new(io::ErrorKind::PermissionDenied, "no installation ownership ID; system trust was left untouched")
	})?;
	let script = root.script(&owner.fingerprint(), &sha256_hex(store.as_bytes()));
	macos::run(&format!("{script}\ncertificate_count={certificate_count}\nremove_trust"), elevation)?;
	if matching_count(&root, &certificates()?)? != 0 {
		return Err(io::Error::other("the root remains in the system keychain"));
	}
	Ok(())
}

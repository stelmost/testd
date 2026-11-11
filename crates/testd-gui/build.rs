fn main() {
	println!("cargo:rerun-if-changed=resources.rc");
	println!("cargo:rerun-if-changed=app.manifest");
	println!("cargo:rerun-if-changed=icons/icon.ico");
	#[cfg(windows)]
	if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
		let version = std::env::var("CARGO_PKG_VERSION").expect("Cargo package version is missing");
		let numeric = version.split(['-', '+']).next().unwrap().replace('.', ",");
		let definitions = [format!("TESTD_FILE_VERSION={numeric},0"), format!("TESTD_VERSION_STRING=\"{version}\"")];
		embed_resource::compile("resources.rc", definitions.as_slice())
			.manifest_required()
			.expect("could not embed the Windows application icon, version and manifest");
	}
}

#![cfg(target_os = "macos")]

#[test]
fn socket_helper_arguments_bypass_desktop_activation() {
	let output = std::process::Command::new(env!("CARGO_BIN_EXE_testd-gui"))
		.arg("--testd-socket-helper")
		.output()
		.unwrap();
	let error = String::from_utf8_lossy(&output.stderr);
	assert_eq!(output.status.code(), Some(1), "{error}");
	assert!(error.contains("socket activation failed: missing socket-transfer path"), "{error}");
}

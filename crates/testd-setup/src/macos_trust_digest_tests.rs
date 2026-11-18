use super::*;

#[test]
fn trust_probe_distinguishes_absence_from_export_errors() {
	let directory = tempfile::tempdir().unwrap();
	let work = shell_quote(&directory.path().to_string_lossy());
	let root = root(directory.path().join("receipt"));
	let script = root.script("fixture-owner", "snapshot");
	let fixture = directory.path().join("export.plist");
	let run = |export: &str| {
		let stub = format!("security() {{ {export}; }}");
		Command::new("sh")
			.args(["-c", &format!("set -eu\n{script}\nwork={work}\n{stub}\ntrust_digest")])
			.output()
			.unwrap()
	};
	let export = format!("cp {} \"$3\"", shell_quote(&fixture.to_string_lossy()));
	std::fs::write(
		&fixture,
		format!(
			"<?xml version=\"1.0\"?><plist version=\"1.0\"><dict><key>trustList</key><dict>\
			<key>{}</key><dict><key>trustSettings</key><array/></dict></dict></dict></plist>",
			root.sha1
		),
	)
	.unwrap();
	let present = run(&export);
	assert!(present.status.success(), "{}", String::from_utf8_lossy(&present.stderr));
	assert_eq!(String::from_utf8(present.stdout).unwrap().trim().len(), 64);
	std::fs::write(&fixture, "<plist version=\"1.0\"><dict><key>trustList</key><dict/></dict></plist>").unwrap();
	let missing_entry = run(&export);
	assert!(missing_entry.status.success());
	assert_eq!(missing_entry.stdout, b"absent\n");
	let empty_store = run("printf 'No Trust Settings were found\\n' >&2; return 1");
	assert!(empty_store.status.success());
	assert_eq!(empty_store.stdout, b"absent\n");
	let denied = run("printf 'authorization denied\\n' >&2; return 1");
	assert!(!denied.status.success());
	assert!(String::from_utf8_lossy(&denied.stderr).contains("authorization denied"));
}

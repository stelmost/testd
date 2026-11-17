#!/usr/bin/env bash
# macOS host checks: resolver, trust, edge, DNS, registration, sharing, launchd.
#
# Run on the target Mac, not in the dev container:
#   cargo build --release -p testd
#   tests/host/macos.sh
#
# System checks run only through tests/run.py with a persisted snapshot and owned cleanup.

set -euo pipefail
cd "$(dirname "$0")/../.."
source tests/host/lib.sh
source tests/host/common.sh
require_bin

check_binary
check_init
check_daemon
check_edge
check_dashboard
check_doctor_state
check_dns
check_registration
check_sharing
check_quick_launch
check_url_actions

section 'resolver and trust'
if [ "${TESTD_HOST_TEST_SYSTEM:-0}" != '1' ] || ! sudo -n true 2>/dev/null; then
	skip 'system changes disabled (run tests/run.py host --system with cached sudo)'
elif ! require_daemon; then
	fail 'system checks require the fixture daemon'
else
	if HOME="$TEST_HOME" "$TESTD_BIN" doctor --fix --json >"$TEST_HOME/doctor.json" 2>"$TEST_HOME/doctor.log"; then
		pass 'doctor --fix'
	else
		fail 'doctor --fix'
	fi
	if python3 - "$TEST_HOME/doctor.json" <<'PY'
import json, sys
checks = {check['name']: check['ok'] for check in json.load(open(sys.argv[1]))['checks']}
assert all(checks.get(name) is True for name in ('resolver', 'resolution', 'certificate', 'edge')), checks
PY
	then
		pass 'resolver, name resolution, root trust, and edge'
	else
		fail 'resolver, name resolution, root trust, or edge'
	fi
	if dscacheutil -q host -a name "testd.$TLD" | grep -q 'ip_address: 127.0.0.1'; then
		pass 'system resolver route'
	else
		fail 'system resolver route'
	fi
	check_system_trust
fi

section 'launchd'
if plutil -lint packaging/launchd/dev.testd.host.plist >/dev/null 2>&1; then
	pass 'plist verifies'
else
	fail 'plist verifies'
fi

check_launch_at_login

section 'cleanup'
printf 'config:      %s\n' "$TEST_HOME"

stop_daemon
summary

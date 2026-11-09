"""Send the dashboard credential to curl over a byte pipe, not in command arguments."""

import base64
import json
import pathlib
import re
import subprocess
import sys


def invoke_dashboard_curl(control, curl, arguments):
	data = json.loads(pathlib.Path(control).read_text(encoding='utf-8-sig'))
	token = data.get('dashboard_token') if isinstance(data, dict) else None
	if not isinstance(token, str) or not re.fullmatch(r'[A-Za-z0-9_-]+', token):
		raise ValueError('invalid dashboard credential')
	header = f'header = "Authorization: Bearer {token}"\n'.encode('ascii')
	return subprocess.run([curl, '--config', '-', *arguments], input=header, timeout=30).returncode


if __name__ == '__main__':
	control, curl, encoded = sys.argv[1:]
	arguments = json.loads(base64.b64decode(encoded, validate=True))
	if not isinstance(arguments, list) or not all(isinstance(argument, str) for argument in arguments):
		raise ValueError('curl arguments must be a list of strings')
	sys.exit(invoke_dashboard_curl(control, curl, arguments))

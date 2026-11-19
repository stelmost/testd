#!/usr/bin/env bash

# Capture the host values before GTK and AppRun change the tray's environment.
export TESTD_APPIMAGE_HOST_ENV=1
for variable in PATH LD_LIBRARY_PATH LD_PRELOAD PYTHONHOME PYTHONPATH PERLLIB GSETTINGS_SCHEMA_DIR \
	GI_TYPELIB_PATH GTK_EXE_PREFIX GTK_DATA_PREFIX GTK_PATH GTK_IM_MODULE_FILE GTK_THEME GDK_BACKEND \
	GDK_PIXBUF_MODULE_FILE XDG_DATA_DIRS; do
	unset "TESTD_APPIMAGE_ORIGINAL_$variable" "TESTD_APPIMAGE_UNSET_$variable"
	if [[ -v "$variable" ]]; then
		export "TESTD_APPIMAGE_ORIGINAL_$variable=${!variable}"
	else
		export "TESTD_APPIMAGE_UNSET_$variable=1"
	fi
done

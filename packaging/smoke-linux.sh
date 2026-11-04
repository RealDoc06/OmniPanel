#!/usr/bin/env bash
# Runs inside a clean container: launches the app under Xvfb, checks that the deck
# server answers and the log is free of crashes. Used by scripts/verify-release.sh.
#   smoke-linux.sh <expected version> <command...>
set -u
version="$1"
shift
log=/tmp/omnipanel.log

# A session bus and an X server stand in for a desktop session.
RUST_LOG=info dbus-run-session -- xvfb-run -a "$@" >"$log" 2>&1 &
app=$!

info=""
for _ in $(seq 1 60); do
  info="$(curl -fsS --max-time 2 http://127.0.0.1:7710/api/info 2>/dev/null)" && break
  info=""
  kill -0 "$app" 2>/dev/null || break
  sleep 1
done
sleep 5 # let a late crash show up

status=0
echo "api/info: ${info:-no answer}"
case "$info" in *"\"version\":\"$version\""*) ;; *) echo "FAIL: wrong or no version"; status=1 ;; esac
kill -0 "$app" 2>/dev/null || { echo "FAIL: app exited early"; status=1; }
if grep -qiE 'panicked|segmentation fault|core dumped|error while loading shared' "$log"; then
  echo "FAIL: crash in log"
  status=1
fi
# Informational: the window opens a web process only when the app shows it at launch.
echo "webkit web processes: $(grep -lsa WebKitWebProcess /proc/[0-9]*/cmdline | wc -l)"
echo "--- log"
tail -"$([ "$status" = 0 ] && echo 8 || echo 40)" "$log"
kill "$app" 2>/dev/null
exit "$status"

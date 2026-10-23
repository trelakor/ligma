#!/bin/zsh
# Memory use of Ligma (or the Figma desktop app) and the processes it's
# responsible for, on macOS. The macOS twin of measure-memory.ps1.
#
# WebKit runs each page in com.apple.WebKit.WebContent processes (plus one
# GPU and one Networking process). launchd starts those, so they aren't
# children of the app. Activity Monitor attributes them by "responsible
# process", and so does this script. Sizes are physical footprint, the
# number Activity Monitor shows as "Memory".
#
#   ./scripts/measure-memory.sh               # Ligma
#   ./scripts/measure-memory.sh -n Figma      # Figma desktop, to compare
#   ./scripts/measure-memory.sh -w 5          # repeat every 5 seconds
#   ./scripts/measure-memory.sh -p 1234       # one instance, by PID

name=ligma
watch=0
pid=
while getopts "n:w:p:" opt; do
  case $opt in
    n) name=$OPTARG ;;
    w) watch=$OPTARG ;;
    p) pid=$OPTARG ;;
    *) exit 2 ;;
  esac
done

measure() {
  local app=${pid:-$(pgrep -x "$name" | head -1)}
  if [[ -z $app ]]; then
    echo "No $name running."
    return
  fi

  # Every process the app is responsible for, itself included.
  # responsibility_get_pid_responsible_for_pid is what Activity Monitor uses.
  # An app started from a terminal (a dev build) isn't responsible for
  # itself: the terminal is, for it and for its WebKit processes. Then count
  # the WebKit processes the terminal is responsible for instead, so close
  # other WebKit apps started from that terminal.
  local pids=($(ps -A -o pid=,comm= | python3 -I -c '
import ctypes, sys
lookup = ctypes.CDLL(None).responsibility_get_pid_responsible_for_pid
lookup.argtypes = [ctypes.c_int]
app = int(sys.argv[1])
owner = lookup(app)
for line in sys.stdin:
    pid, command = line.split(None, 1)
    pid = int(pid)
    responsible = lookup(pid)
    if pid == app or responsible == app or (
        owner != app and responsible == owner and "/com.apple.WebKit." in command
    ):
        print(pid)
' "$app"))

  local args=()
  local p
  for p in $pids; do args+=(-p $p); done

  # footprint prints "<name> [<pid>]: 64-bit    Footprint: <bytes> B" per process.
  footprint --noCategories -f bytes $args 2>/dev/null | awk -v name="$name" '
    / Footprint: / && /\[[0-9]+\]/ {
      match($0, /\[[0-9]+\]/)
      pid = substr($0, RSTART + 1, RLENGTH - 2)
      process = substr($0, 1, RSTART - 2)
      sub(/^com\.apple\.WebKit\./, "", process)
      for (i = 1; i <= NF; i++) if ($i == "Footprint:") bytes = $(i + 1)
      mb = bytes / 1048576
      printf "%8.1f MB  %-8s %s\n", mb, pid, process | "sort -rn"
      total += mb
      count++
    }
    END {
      close("sort -rn")
      printf "%s: %d MB in %d processes\n", name, total, count
    }
  '
}

while true; do
  measure
  (( watch > 0 )) || break
  sleep $watch
  echo
done

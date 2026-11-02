#!/bin/bash
# exit 0 once the game process has run for 100s and RCON is still closed (settings line unparsable, defaults in force)
for i in $(seq 1 150); do
  e=$(ps -eo etimes,args | awk '/[S]hipping/{print $1; exit}')
  [ -n "$e" ] && [ "$e" -ge 100 ] && break
  sleep 4
done
[ "${e:-0}" -ge 100 ] || exit 1
(exec 3<>/dev/tcp/127.0.0.1/25575) 2>/dev/null && exit 1
exit 0

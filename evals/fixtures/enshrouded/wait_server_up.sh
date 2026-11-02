#!/bin/bash
# exit 0 once the Enshrouded server process has run for 30s on a parsable config
for i in $(seq 1 100); do
  e=$(ps -eo etimes,comm | awk '/enshrouded_serv/{print $1; exit}')
  [ -n "$e" ] && [ "$e" -ge 30 ] && jq -e . /home/steam/enshrouded/enshrouded_server.json >/dev/null 2>&1 && exit 0
  sleep 5
done
exit 1

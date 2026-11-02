#!/bin/bash
# exit 0 once the Palworld RCON port accepts connections (the game is up with a parsed settings line)
for i in $(seq 1 120); do
  (exec 3<>/dev/tcp/127.0.0.1/25575) 2>/dev/null && exit 0
  sleep 4
done
exit 1

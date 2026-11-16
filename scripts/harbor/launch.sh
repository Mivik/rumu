#!/bin/sh
# Starts `rumu acp` inside a Harbor task container: a fresh RUMU_HOME per
# run, seeded from /opt/rumu/home (put a settings.yaml with a model there).
export RUMU_HOME="${RUMU_HOME:-/tmp/rumu-home}"
if [ ! -d "$RUMU_HOME" ]; then
  mkdir -p "$RUMU_HOME"
  if [ -d /opt/rumu/home ]; then
    cp -R /opt/rumu/home/. "$RUMU_HOME/"
  fi
fi
exec /opt/rumu/rumu acp "$@"

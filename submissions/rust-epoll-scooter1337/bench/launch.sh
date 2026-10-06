#!/usr/bin/env bash
set -euo pipefail
variant=${1:?variant}
cp /bench/seed/feed.db /tmp/feed.db
chown bench:bench /tmp/feed.db
if [[ $variant == pr2 || $variant == pr12 ]]; then
  cat >/tmp/nginx.conf <<'CONF'
worker_processes 1;
worker_rlimit_nofile 65535;
user bench;
pid /tmp/nginx.pid;
events { worker_connections 16384; }
http {
  include /etc/nginx/mime.types;
  default_type application/octet-stream;
  keepalive_timeout 75s;
  include /bench/all/challenge/bench/nginx.conf;
}
CONF
  runuser -u bench --preserve-environment -- bash "/bench/all/source/$variant/start.sh" > /tmp/app.log 2>&1 &
  backend=$!
  trap 'kill "$backend" 2>/dev/null || true' EXIT
  for ((i=0;i<600;i++)); do
    if curl -fsS http://127.0.0.1:3000/health >/dev/null 2>&1; then break; fi
    kill -0 "$backend" || { cat /tmp/app.log; exit 1; }
    sleep .1
  done
  nginx -c /tmp/nginx.conf -g 'daemon off;' &
  proxy=$!
  wait -n "$backend" "$proxy"
else
  exec runuser -u bench --preserve-environment -- bash "/bench/all/source/$variant/start.sh"
fi

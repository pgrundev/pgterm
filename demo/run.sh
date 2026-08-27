#!/bin/sh
# Demo pgterm with three pretend databases — no PostgreSQL, no pgbot needed.
# Uses its own config under $TMPDIR; your real ~/.config/pgterm is untouched.
set -eu
here="$(cd "$(dirname "$0")" && pwd)"
demo_home="${TMPDIR:-/tmp}/pgterm-demo"
mkdir -p "$demo_home"
export PGTERM_CONFIG="$demo_home/config.toml"
export PGBOT_BIN="$here/fake-pgbot"
export DEMO_PROD_URL='postgres://demo@mode-healthy.local/app'
export DEMO_STAGING_URL='postgres://demo@mode-warn.local/app'
export DEMO_ANALYTICS_URL='postgres://demo@mode-critical.local/app'
bin="$here/../target/release/pgterm"
[ -x "$bin" ] || bin="$here/../target/debug/pgterm"
[ -x "$bin" ] || { echo "build pgterm first: cargo build --release"; exit 1; }
if [ ! -f "$PGTERM_CONFIG" ]; then
  "$bin" add production --env DEMO_PROD_URL
  "$bin" add staging    --env DEMO_STAGING_URL
  "$bin" add analytics  --env DEMO_ANALYTICS_URL
fi
exec "$bin" --interval 15s

#!/usr/bin/env bash
# Quick start script for ekapkgs-update-web

set -euo pipefail

# Default values
DATABASE="${EKAPKGS_DATABASE:-$HOME/.local/share/ekapkgs-update/db.sqlite}"
PORT="${EKAPKGS_WEB_PORT:-3000}"
HOST="${EKAPKGS_WEB_HOST:-127.0.0.1}"
CORS="${EKAPKGS_WEB_CORS:-}"

echo "🚀 Starting ekapkgs-update-web..."
echo "   Database: $DATABASE"
echo "   URL: http://$HOST:$PORT"
echo ""
if [ ! -f "$DATABASE" ]; then
    echo "ℹ️  Database will be created automatically (currently empty)"
    echo "   Run 'ekapkgs-update run' to populate with update data"
    echo ""
fi
echo "Press Ctrl+C to stop"
echo ""

# Build CORS flag
CORS_FLAG=""
if [ -n "$CORS" ]; then
    CORS_FLAG="--cors"
    echo "⚠️  CORS enabled (public access allowed)"
fi

# Start the server
exec cargo run -p ekapkgs-update-web -- \
    --database "$DATABASE" \
    --host "$HOST" \
    --port "$PORT" \
    $CORS_FLAG

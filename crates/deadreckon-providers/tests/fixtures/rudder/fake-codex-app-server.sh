#!/bin/sh
set -eu

mode="${2:-normal}"

while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
      if [ "$mode" = "handshake-failure" ]; then
        printf '{"id":%s,"error":{"code":-32000,"message":"fixture initialize failed"}}\n' "$id"
      else
        printf '{"id":%s,"result":{"userAgent":"fake-codex","codexHome":"/tmp/fake-codex","platformFamily":"unix","platformOs":"test"}}\n' "$id"
      fi
      ;;
    *'"method":"initialized"'*)
      ;;
  esac
done

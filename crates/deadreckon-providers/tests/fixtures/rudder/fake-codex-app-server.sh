#!/bin/sh
set -eu

mode="${2:-normal}"
log="${3:-}"

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
    *'"method":"thread/start"'*)
      id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
      if [ -n "$log" ]; then printf 'thread/start\n' >> "$log"; fi
      printf '{"id":%s,"result":{"thread":{"id":"thread-fixture"}}}\n' "$id"
      ;;
    *'"method":"thread/resume"'*)
      id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
      if [ -n "$log" ]; then printf 'thread/resume\n' >> "$log"; fi
      printf '{"id":%s,"result":{"thread":{"id":"thread-fixture"}}}\n' "$id"
      ;;
    *'"method":"turn/start"'*)
      id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
      if [ -n "$log" ]; then printf 'turn/start\n' >> "$log"; fi
      printf '{"id":%s,"result":{"turn":{"id":"turn-fixture","status":"inProgress","items":[]}}}\n' "$id"
      if [ "$mode" = "turn-failed" ]; then
        printf '{"method":"turn/completed","params":{"threadId":"thread-fixture","turn":{"id":"turn-fixture","status":"failed","items":[],"error":{"message":"fixture turn failed"}}}}\n'
      else
        printf '{"method":"thread/tokenUsage/updated","params":{"threadId":"thread-fixture","turnId":"turn-fixture","tokenUsage":{"total":{"inputTokens":321,"cachedInputTokens":0,"outputTokens":45,"reasoningOutputTokens":0,"totalTokens":366},"last":{"inputTokens":321,"cachedInputTokens":0,"outputTokens":45,"reasoningOutputTokens":0,"totalTokens":366},"modelContextWindow":258400}}}\n'
        printf '{"method":"item/completed","params":{"threadId":"thread-fixture","turnId":"turn-fixture","item":{"type":"agentMessage","id":"item-fixture","text":"fixture answer"}}}\n'
        printf '{"method":"turn/completed","params":{"threadId":"thread-fixture","turn":{"id":"turn-fixture","status":"completed","items":[]}}}\n'
      fi
      ;;
  esac
done

#!/bin/sh
set -eu

if [ "${1:-}" != "app-server" ]; then
  printf '{"type":"thread.started","thread_id":"fallback-thread"}\n'
  printf '{"type":"item.completed","item":{"id":"fallback-item","type":"agent_message","text":"{\\"action\\":\\"done\\",\\"summary\\":\\"exec fallback completed\\"}"}}\n'
  printf '{"type":"turn.completed","usage":{"input_tokens":7,"cached_input_tokens":0,"output_tokens":3,"reasoning_output_tokens":0}}\n'
  exit 0
fi

mode="${2:-normal}"
log="${3:-}"
turn_count=0
steer_count=0

if [ -n "$log" ]; then
  trap 'printf "process/killed\n" >> "$log"' TERM INT EXIT
fi

emit_turn_completion() {
  completed_turn_id="$1"
  if [ "$mode" = "turn-failed" ]; then
    printf '{"method":"turn/completed","params":{"threadId":"thread-fixture","turn":{"id":"%s","status":"failed","items":[],"error":{"message":"fixture turn failed"}}}}\n' "$completed_turn_id"
  else
    printf '{"method":"thread/tokenUsage/updated","params":{"threadId":"thread-fixture","turnId":"%s","tokenUsage":{"total":{"inputTokens":321,"cachedInputTokens":0,"outputTokens":45,"reasoningOutputTokens":0,"totalTokens":366},"last":{"inputTokens":321,"cachedInputTokens":0,"outputTokens":45,"reasoningOutputTokens":0,"totalTokens":366},"modelContextWindow":258400}}}\n' "$completed_turn_id"
    printf '{"method":"item/completed","params":{"threadId":"thread-fixture","turnId":"%s","item":{"type":"agentMessage","id":"item-fixture","text":"fixture answer"}}}\n' "$completed_turn_id"
    printf '{"method":"turn/completed","params":{"threadId":"thread-fixture","turn":{"id":"%s","status":"completed","items":[]}}}\n' "$completed_turn_id"
  fi
}

while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
      if [ -n "$log" ]; then printf 'initialize\n' >> "$log"; fi
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
      turn_count=$((turn_count + 1))
      turn_id="turn-fixture"
      if [ "$mode" = "stale-steer-once" ]; then
        turn_id="turn-stale-$turn_count"
      fi
      if [ -n "$log" ]; then printf 'turn/start\n' >> "$log"; fi
      printf '{"id":%s,"result":{"turn":{"id":"%s","status":"inProgress","items":[]}}}\n' "$id" "$turn_id"
      if [ "$mode" = "approval-command" ]; then
        printf '{"id":"approval-1","method":"item/commandExecution/requestApproval","params":{"threadId":"thread-fixture","turnId":"%s","itemId":"command-1","startedAtMs":1,"command":"curl https://api.example.com/v1","cwd":"/workspace"}}\n' "$turn_id"
      elif [ "$mode" = "die-mid-turn" ]; then
        exit 23
      elif [ "$mode" != "wait-for-steer" ] && [ "$mode" != "wait-for-interrupt" ]; then
        emit_turn_completion "$turn_id"
      fi
      ;;
    *'"method":"turn/interrupt"'*)
      id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
      if [ -n "$log" ]; then printf 'turn/interrupt\n' >> "$log"; fi
      printf '{"id":%s,"result":{}}\n' "$id"
      emit_turn_completion "turn-fixture"
      ;;
    *'"id":"approval-1"'*)
      if [ -n "$log" ]; then printf '%s\n' "$line" >> "$log"; fi
      emit_turn_completion "turn-fixture"
      ;;
    *'"method":"turn/steer"'*)
      id=$(printf '%s\n' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
      expected_turn_id=$(printf '%s\n' "$line" | sed -n 's/.*"expectedTurnId":"\([^"]*\)".*/\1/p')
      steer_count=$((steer_count + 1))
      if [ -n "$log" ]; then printf '%s\n' "$line" >> "$log"; fi
      if [ "$mode" = "stale-steer-once" ] && [ "$steer_count" -eq 1 ]; then
        printf '{"id":%s,"error":{"code":-32600,"message":"no active turn to steer"}}\n' "$id"
      else
        printf '{"id":%s,"result":{"turnId":"%s"}}\n' "$id" "$expected_turn_id"
      fi
      if [ "$mode" = "wait-for-steer" ]; then
        emit_turn_completion "$expected_turn_id"
      fi
      ;;
  esac
done

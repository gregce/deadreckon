#!/bin/sh
mkdir -p .deadreckon-seams
cat >> .deadreckon-seams/event-sink.jsonl
printf '%s\n' '{"ok":true}'

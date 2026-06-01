#!/bin/sh
mkdir -p .deadreckon-seams
cat >> .deadreckon-seams/hooks.jsonl
printf '%s\n' '{"ok":true}'

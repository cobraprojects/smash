#!/bin/bash

# Older Smash builds advertise the upstream protocol names.
protocol="${SMASH_CLI_AGENT_PROTOCOL_VERSION:-${WARP_CLI_AGENT_PROTOCOL_VERSION:-}}"
client="${SMASH_CLIENT_VERSION:-${WARP_CLIENT_VERSION:-}}"
case "$protocol" in
    ''|*[!0-9]*|0) exit 0 ;;
esac
[ -n "$client" ] || exit 0
command -v jq >/dev/null 2>&1 || exit 0

case "${1:-}" in
    session_start|prompt_submit|permission_request|tool_complete|stop) ;;
    *) exit 0 ;;
esac

plugin_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)" || exit 0
payload=$(jq -c --arg event "$1" \
    --slurpfile manifest "$plugin_root/.codex-plugin/plugin.json" '
    def clip($limit):
        if type != "string" then error("expected text")
        elif length > $limit then .[0:($limit - 3)] + "..." else . end;
    select(type == "object") |
    {v: 1, agent: "codex", event: $event,
     session_id: (.session_id // ""), cwd: (.cwd // ""),
     project: (((.cwd // "") | rtrimstr("/") | split("/") | last) // "")} +
    (if $event == "session_start" then {plugin_version: $manifest[0].version}
     elif $event == "prompt_submit" then {query: ((.prompt // "") | clip(200))}
     elif $event == "stop" then
         {response: ((.last_assistant_message // "") | clip(200)),
          transcript_path: (.transcript_path // "")}
     elif $event == "tool_complete" then {tool_name: (.tool_name // "")}
     else
         (.tool_name // "unknown") as $tool |
         (.tool_input // {}) as $input |
         (($input.command // $input.file_path // ($input | tostring)) | tostring | clip(120)) as $preview |
         {tool_name: $tool, tool_input: $input,
          summary: ("Wants to run " + $tool + if $preview == "" then "" else ": " + $preview end)}
     end)
' 2>/dev/null) || exit 0
[ -n "$payload" ] || exit 0

sentinel="smash://cli-agent"
if [ -z "${SMASH_CLI_AGENT_PROTOCOL_VERSION:-}" ]; then
    sentinel="warp://cli-agent"
fi
# Never print hook output into the model conversation or fail the agent's turn.
{ printf '\033]777;notify;%s;%s\007' "$sentinel" "$payload" > /dev/tty; } 2>/dev/null || true

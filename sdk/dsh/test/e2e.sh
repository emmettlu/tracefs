#!/bin/bash
# dsh-acfs in a real DeepSeek Harness: the headless profile with this plugin, its scripted mock model calling bash in an
# acfs-lite branch. Checks the changeset of the tool call and the merge into main when the turn ends.
#   DSH_SOURCE=<built deepseek-harness checkout> bash sdk/dsh/test/e2e.sh
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
dsh=${DSH_SOURCE:?set DSH_SOURCE to a built deepseek-harness checkout}
cli=$here/../../portable/src/cli.js
port=${PORT:-8765}
work=$(mktemp -d)
export ACFS_LITE_ROOT=$work/store
mock=
cleanup() {
	[ -n "$mock" ] && kill "$mock" 2>/dev/null || true
	rm -rf "$work"
}
trap cleanup EXIT

node "$cli" register app >/dev/null
branch=$(node "$cli" branch app agent)
cat >"$work/overlay.yml" <<EOF
- insert:
    - id: acfs
      name: '$here/../index.js'
EOF

(cd "$dsh" && exec node --import tsx packages/test-support/llm-mock-server/src/bin.ts --port "$port" --api-key mock-key \
	--sequence tool_call_success,success --repeat-last --tool-name bash \
	--tool-arguments '{"command":"echo hi > hello.txt","description":"write hello"}' --success-text done) >"$work/mock.log" 2>&1 &
mock=$!
for _ in $(seq 100); do grep -q '"ready"' "$work/mock.log" 2>/dev/null && break; sleep 0.2; done
grep -q '"ready"' "$work/mock.log" || { cat "$work/mock.log"; exit 1; }

limit=()
if command -v timeout >/dev/null; then limit=(timeout 300); elif command -v gtimeout >/dev/null; then limit=(gtimeout 300); fi
status=0
(cd "$branch" && TSX_TSCONFIG_PATH=$dsh/tsconfig.json DSH_HOME=$work/home DEEPSEEK_BASE_URL=http://127.0.0.1:$port/v1 DEEPSEEK_API_KEY=mock-key \
	${limit[@]+"${limit[@]}"} node --import "file://$dsh/node_modules/tsx/dist/esm/index.mjs" "$dsh/apps/cli/src/bin.ts" \
	--profile headless --patch "$work/overlay.yml" --json "write hello") >"$work/dsh.out" 2>"$work/dsh.err" || status=$?

fail=0
check() {
	if eval "$2"; then echo "ok - $1"; else echo "not ok - $1"; fail=1; fi
}
check "dsh exits 0" '[ "$status" = 0 ]'
check "the bash call ran" 'grep -q "\"type\":\"tool_result\",\"callId\":\"mock-call-1\",\"status\":\"completed\"" "$work/dsh.out"'
check "it wrote the branch" '[ "$(cat "$branch/hello.txt" 2>/dev/null)" = hi ]'
log=$(node "$cli" log app)
check "its changeset is the dsh bash call" 'grep -q "app/agent dsh bash: echo hi > hello.txt" <<<"$log"'
check "the turn end merged it into main" 'grep -q "app/main dsh merge agent" <<<"$log" && [ "$(cat "$ACFS_LITE_ROOT/trees/app/main/hello.txt" 2>/dev/null)" = hi ]'
check "the changeset carries the tool call id" 'node "$cli" log app --json | grep -q "\"tool_call\": \"mock-call-1\""'
if [ $fail != 0 ]; then
	echo "--- log"; echo "$log"; echo "--- dsh stderr"; tail -c 3000 "$work/dsh.err"; echo "--- dsh events"; tail -c 3000 "$work/dsh.out"
fi
exit $fail

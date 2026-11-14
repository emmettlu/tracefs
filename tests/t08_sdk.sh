#!/bin/bash
# SDK clients (sdk/python, sdk/typescript) against the daemon; every result checked against the schema.
. "$(dirname "$0")/lib.sh"
setup

D=$(cd "$(dirname "$0")" && pwd)
SCHEMA=$D/../schema/acfs.schema.json
W=$(mkws w)
afs ws register w --from "$W" >/dev/null
export ACFS_SOCKET=$SOCK W PYTHONDONTWRITEBYTECODE=1

ok 8.1 "python sdk: workspace and session cycle, error codes" python3 $D/sdk_smoke.py $T/py.log py.txt
ok 8.1a "python sdk results match the schema" python3 $D/schema_check.py $SCHEMA $T/py.log
TSRUN=$(command -v node || command -v bun)
if [ -n "$TSRUN" ]; then
	ok 8.2 "typescript sdk ($(basename $TSRUN)): workspace and session cycle, error codes" $TSRUN $D/sdk_smoke.mts $T/ts.log ts.txt
	ok 8.2a "typescript sdk results match the schema" python3 $D/schema_check.py $SCHEMA $T/ts.log
else
	res 8.2 SKIP "neither node nor bun installed"
fi
ok 8.3 "the SDK writes reached the directory" bash -c "grep -qx sdk $W/py.txt && { [ -z '$TSRUN' ] || grep -qx sdk $W/ts.txt; }"
finish_test

#!/bin/bash
# Benchmarks of acfs itself (AGENTS.md 8); run through run.sh: sudo bash tests/run.sh bench
# Writes a Markdown table to tests/out/bench.md. Data stays on disk ($TROOT).
. "$(dirname "$0")/lib.sh"
setup 32G
MD=${BENCH_MD:-$(dirname "$(readlink -f "$0")")/out/bench.md}
restart() { stop_daemon; DIFF_ENV=$1 start_daemon "${@:2}"; }
restart ACFS_BENCH=1
ROWS=()
row() { ROWS+=("| $1 | $2 |"); res "$1" INFO "$2"; }
us() { echo $(($(date +%s%N) / 1000)); }
# Milliseconds with one decimal from microseconds.
show() { printf '%d.%d' $(($1 / 1000)) $(($1 % 1000 / 100)); }
# t CMD...: time one command; a failure is a FAIL result, not a number.
t() {
	local t0 rc
	t0=$(us)
	"$@" >$T/t.out 2>&1
	rc=$?
	[ $rc = 0 ] || res bench FAIL "$*: $(tail -c 300 $T/t.out)"
	show $(($(us) - t0))
}

# A 50k-file tree (20 directories x 2500 files of 1 KiB) and a git repository.
W=$T/src/big
for d in $(seq 1 20); do
	mkdir -p $W/d$d
	for f in $(seq 1 2500); do head -c 1024 /dev/zero >$W/d$d/f$f; done
done
chown -R $AGENT:$AGENT $W
row "register, 50k files" "$(t afs ws register big --from $W) ms"
RG=$(mkrepo rg)
afs ws register rg --from $RG >/dev/null
SM=$(mkws small) && afs ws register small --from $SM >/dev/null
t0=$(us)
P=$(spawn --ws big --ws rg --ws small)
row "acfs run until the command starts, 3 workspaces" "$(show $(($(us) - t0))) ms"
PB=$RUN/mnt/$(branch $P big)
for d in $(seq 1 4); do for f in $(seq 1 2500); do echo x >>$PB/d$d/f$f; done; done
t afs proc changeset $P --author b -m 10k >/dev/null
CS1=$(j "d['changeset']" <$T/t.out)
row "proc changeset, 10k changed files (pause)" "$(j "round(d['pause_ms'], 1)" <$T/t.out) ms"
for f in $(seq 1 10); do echo y >>$PB/d9/f$f; done
CS2=$(afs proc changeset $P --author b -m 10 | j "d['changeset']")
d1=$(t afs diff $CS1) d2=$(t afs diff $CS2)
restart ACFS_DIFF_V1=1
v1=$(t afs diff $CS1) v2=$(t afs diff $CS2)
restart ACFS_BENCH=1
row "diff of the 10k-file changeset (v2 / v1)" "$d1 / $v1 ms"
row "diff of a 10-file changeset after it (v2 / v1)" "$d2 / $v2 ms"
row "proc merge: 10k changed files into main" "$(t afs proc merge $P) ms"
P2=$(spawn --ws big)
XDIR=$W x $P2 "mv d5 d5moved"
row "proc merge: a renamed directory of 2500 files" "$(t afs proc merge $P2) ms"
quit $P >/dev/null; quit $P2 >/dev/null

# One more write after a long history (the auto snapshot's change check).
L=$T/src/hist && mkdir -p $L && cp -a $W/d1 $W/d2 $W/d3 $W/d4 $L/ && chown -R $AGENT:$AGENT $L
afs ws register hist --from $L >/dev/null
for d in 1 2 3 4; do for f in $(seq 1 1250); do echo z >>$L/d$d/f$f; done; done
afs changeset hist -m history >/dev/null
echo w >>$L/d1/f1
c2=$(t afs changeset hist -m one)
echo w >>$L/d1/f2
restart ACFS_DIFF_V1=1
c1=$(t afs changeset hist -m one)
restart ACFS_BENCH=1
row "changeset of 1 write after 5k recorded changes (v2 / v1)" "$c2 / $c1 ms"

# File access without a mount, 50 calls each (each is a CLI process).
avg() {
	local n=$1 t0 bad=0
	shift
	t0=$(us)
	for _ in $(seq 1 $n); do "$@" >/dev/null 2>$T/avg.err || bad=$((bad + 1)); done
	[ $bad = 0 ] || res bench FAIL "$*: $bad of $n failed: $(tail -c 200 $T/avg.err)"
	show $((($(us) - t0) / n))
}
row "fs cat (mean of 50)" "$(avg 50 afs fs cat hist d1/f1) ms"
row "fs cat of a recorded state (mean of 50)" "$(avg 50 afs fs cat small README --at $(afs ws list | j "[w['head_snap'] for w in d if w['name']=='small'][0]")) ms"
i=0
fsw() { i=$((i + 1)); echo $i | afs fs write "$1" bench/f$i; }
row "fs write (mean of 50)" "$(avg 50 fsw hist) ms"

{
	echo "# acfs benchmarks"
	echo
	echo "$(uname -r), $(nproc) CPUs, $(free -g | awk '/Mem/{print $2}') GiB, state on a loop btrfs image; $(date -u +%F)."
	echo
	echo "| operation | time |"
	echo "| --- | --- |"
	printf '%s\n' "${ROWS[@]}"
} >"$MD"
finish_test

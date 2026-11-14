# Shared helpers for tracefs integration tests. Run as root on a btrfs-capable host.
set -u
SRC=${SRC:-/home/emmett/tracefs-src}
BIN=${BIN:-$SRC/target/release}
SLICE=afstest
AGENT=1000 # uid of the agent processes; mkws trees belong to it
NAME=${NAME:-$(basename "$0" .sh)}
# Test state lives on disk, never on tmpfs: a loop image in /tmp is host RAM.
# Outside /home so other uids can traverse it.
TROOT=${TROOT:-/var/tmp/afst}
T=$TROOT/$NAME
RUN=/run/afst-$NAME
SOCK=$RUN/tracefs-daemon.sock
STATE=$T/state
LOG=$T/daemon.log
PASS=0
FAIL=0

res() { printf 'RESULT|%s|%s|%s\n' "$1" "$2" "$3"; [ "$2" = PASS ] && PASS=$((PASS + 1)); [ "$2" = FAIL ] && FAIL=$((FAIL + 1)); true; }
ok() { # ok ID DESC CMD...
	local id=$1 desc=$2
	shift 2
	if "$@" >$TROOT/last-$NAME.out 2>&1; then res "$id" PASS "$desc"; else res "$id" FAIL "$desc: $(tail -c 400 $TROOT/last-$NAME.out | tr '\n' ' ')"; fi
}
bad() { # bad ID DESC CMD... (expects failure)
	local id=$1 desc=$2
	shift 2
	if "$@" >$TROOT/last-$NAME.out 2>&1; then res "$id" FAIL "$desc: unexpectedly succeeded"; else res "$id" PASS "$desc: $(tail -c 200 $TROOT/last-$NAME.out | tr '\n' ' ')"; fi
}
afs() { "$BIN/tracefs" --socket "$SOCK" "$@"; }
j() { python3 -c "import json,sys; d=json.load(sys.stdin); print(eval(sys.argv[1], {'d': d}))" "$1"; }
ms() { echo $(($(date +%s%N) / 1000000)); }
export BIN SOCK STATE RUN T TROOT AGENT
export -f afs j ms

cleanup() {
	pkill -f "tracefsd --state-dir $STATE" 2>/dev/null
	sleep 0.2
	systemctl kill -s KILL "$SLICE.slice" >/dev/null 2>&1
	sleep 0.2
	for m in $(findmnt -rn -o TARGET | grep -E "^($RUN|$T)(/|$)" | sort -r); do umount -l "$m" 2>/dev/null; done
	systemctl stop "$SLICE.slice" >/dev/null 2>&1
	rm -rf "$RUN"
	[ -f "$T/state.img" ] && rm -f "$T/state.img"
}

mkstate() { # fresh loop btrfs for the state dir
	mkdir -p "$T"
	chattr +C "$T" 2>/dev/null # no CoW for the image on a btrfs host
	truncate -s "${1:-16G}" "$T/state.img"
	mkfs.btrfs -q -f "$T/state.img"
	mkdir -p "$STATE"
	mount -o loop,noatime "$T/state.img" "$STATE"
}

start_daemon() { # extra args passed through; WSROOT= checks workspaces against the caller's home
	local root=${WSROOT-$TROOT}
	# No automatic snapshots unless a test asks for them: they would interleave with the
	# changesets the tests take and compare.
	case " $* " in *" --auto-quiet-ms"*) ;; *) set -- --auto-quiet-ms 3600000 --auto-max-ms 3600000 "$@" ;; esac
	# NOTIFY_SOCKET as under the systemd unit (Type=notify); nothing listens on it.
	# DIFF_ENV: by default every diff v2 is also computed as v1 and must agree.
	env ${DIFF_ENV:-TRACEFS_DIFF_CHECK=1} NOTIFY_SOCKET=$RUN/notify.sock "$BIN/tracefsd" --state-dir "$STATE" --run-dir "$RUN" --slice $SLICE --stop-timeout-ms 2000 ${root:+"--workspace-root=$root"} "$@" >>"$LOG" 2>&1 &
	DPID=$!
	for _ in $(seq 1 100); do [ -S "$SOCK" ] && afs ping >/dev/null 2>&1 && return 0; sleep 0.05; done
	echo "daemon did not start"; tail -5 "$LOG"; return 1
}
stop_daemon() { kill "$DPID" 2>/dev/null; wait "$DPID" 2>/dev/null; }

setup() {
	cleanup
	rm -rf "$T"
	mkdir -p "$T"
	chmod 755 "$TROOT" "$T"
	mkstate "${1:-16G}"
	start_daemon
}

finish_test() {
	stop_daemon
	# Every diff of two snapshots on one base must have used v2.
	local n
	n=$(grep -c 'walking the uppers' "$LOG" 2>/dev/null)
	if [ "${n:-0}" = 0 ]; then res v2 PASS "no diff fell back to walking the uppers"; else res v2 FAIL "$n diffs fell back: $(grep -m1 'walking the uppers' "$LOG")"; fi
	cleanup
	rm -rf "$T"
	echo "SUMMARY|$NAME|pass=$PASS|fail=$FAIL"
}

# mkws NAME -> workspace source dir owned by $AGENT with a small tree
mkws() {
	local d=$T/src/$1
	mkdir -p "$d/src" "$d/docs"
	for i in $(seq 1 20); do printf 'line %d\n' $(seq 1 30) | sed "s/^/$i /" >"$d/src/f$i.txt"; done
	echo readme >"$d/README"
	echo doc >"$d/docs/a.md"
	chown -R $AGENT:$AGENT "$d"
	echo "$d"
}

# Agent processes (tracefs run) as uid $AGENT.
arun() { setpriv --reuid=$AGENT --regid=$AGENT --clear-groups "$BIN/tracefs" --socket "$SOCK" run "$@"; }
# (arunx ...) &: the same, the background job's pid being tracefs run itself.
arunx() { exec setpriv --reuid=$AGENT --regid=$AGENT --clear-groups "$BIN/tracefs" --socket "$SOCK" run "$@"; }
# spawn [run options...]: start an idle agent from the current directory, print its process
# id. It runs until `quit ID`; `x ID CMD` runs CMD inside it.
spawn() {
	local d=$T/ag tag
	mkdir -p $d && chmod 777 $d
	tag=$d/$(date +%s%N)$RANDOM
	(
		arun "$@" -- sh -c "echo \$TRACEFS_PROC >$tag.id; while [ ! -e $tag.stop ]; do sleep 0.2; done" >$tag.out 2>$tag.err
		echo $? >$tag.rc
	) >/dev/null 2>&1 &
	echo $! >$tag.sub
	for _ in $(seq 1 200); do [ -s $tag.id ] && break; [ -e $tag.rc ] && break; sleep 0.05; done
	cat $tag.id 2>/dev/null || { cat $tag.err >&2; return 1; }
}
tag_of() { local f; f=$(grep -l "^$1\$" $T/ag/*.id) && echo "${f%.id}"; }
# quit ID: let the agent exit, wait for tracefs run to return (its work is merged), print its status.
quit() {
	local tag
	tag=$(tag_of "$1") || return 1
	touch $tag.stop
	for _ in $(seq 1 600); do [ -e $tag.rc ] && break; sleep 0.05; done
	cat $tag.rc
}
pid_of() { afs proc show "$1" | j "d['pid']"; }
# x ID CMD: run CMD (bash) inside agent ID's view, as the agent's uid, in ${XDIR:-$W}.
x() { local p=$1; shift; nsenter -t "$(pid_of $p)" -m setpriv --reuid=$AGENT --regid=$AGENT --clear-groups bash -c "cd ${XDIR:-$W} && $*"; }
# xr ID CMD: the same as root (chown and the like).
xr() { local p=$1; shift; nsenter -t "$(pid_of $p)" -m bash -c "cd ${XDIR:-$W} && $*"; }
# branch ID WS: the agent's branch of WS (mounted at $RUN/mnt/<branch> while it runs).
branch() { afs proc show "$1" | j "[b['branch'] for b in d['branches'] if b['ws']=='$2'][0]"; }
pstate() { afs proc show "$1" | j "d['state']"; }
bstate() { afs proc show "$1" | j "[b['state'] for b in d['branches'] if b['ws']=='$2'][0]"; }
export -f arun arunx spawn tag_of quit pid_of x xr branch pstate bstate

# mkrepo NAME -> a git repository (.git included) of a few thousand files, owned by $AGENT
mkrepo() {
	local d=$T/src/$1 i
	mkdir -p "$d"
	for i in $(seq 1 40); do mkdir -p "$d/crates/c$i/src" && for k in $(seq 1 40); do printf 'fn f%d() {}\n' $k >"$d/crates/c$i/src/m$k.rs"; done; done
	echo readme >"$d/README.md"
	git -C "$d" init -q && git -C "$d" add -A && git -C "$d" -c user.name=t -c user.email=t@t commit -qm one
	echo more >>"$d/README.md" && git -C "$d" -c user.name=t -c user.email=t@t commit -qam two
	chown -R $AGENT:$AGENT "$d"
	echo "$d"
}

# Fingerprint of a tree: path, type, mode, owner, symlink target, content hash, user xattrs.
fp() {
	python3 - "$1" <<'PY'
import hashlib, os, stat, sys
root = sys.argv[1]
out = []
for d, dirs, files in os.walk(root):
    for n in dirs + files:
        p = os.path.join(d, n); st = os.lstat(p); rel = os.path.relpath(p, root); extra = ""
        if stat.S_ISREG(st.st_mode):
            extra = hashlib.sha1(open(p, "rb").read()).hexdigest()
        elif stat.S_ISLNK(st.st_mode):
            extra = os.readlink(p)
        try:
            xs = sorted(x for x in os.listxattr(p, follow_symlinks=False) if x.startswith("user."))
            extra += " " + ",".join(x + "=" + os.getxattr(p, x, follow_symlinks=False).hex() for x in xs)
        except OSError:
            pass
        out.append("%r %o %d:%d %s" % (rel, st.st_mode, st.st_uid, st.st_gid, extra))
print("\n".join(sorted(out)))
PY
}
export -f fp

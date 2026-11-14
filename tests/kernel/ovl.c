// Minimal helper around the new mount API for acfs verification.
//
// ovl [options] --target PATH
//   -l DIR        lowerdir+ (repeatable, first = top)
//   -L DIR        lowerdir+ passed as an fd (FSCONFIG_SET_FD)
//   -u DIR -w DIR upperdir / workdir
//   -B DIR        branch dir: DIR/upper, DIR/work passed as fds from one (idmapped) tree
//   -o K=V | -o K option (repeatable)
//   -r            read-only mount (MOUNT_ATTR_RDONLY)
//   -n PID        setns into PID's mount ns before move_mount
//   -i PID        idmap the resulting overlay mount with PID's userns
//   -I PID        idmap each fd layer (-L/-U/-W) with PID's userns first
//   -t PATH       target (required)
//   -m            use legacy mount(2) instead (string options)
// bind-idmap SRC DST PID: idmapped bind mount of SRC at DST
#define _GNU_SOURCE
#include <errno.h>
#include <fcntl.h>
#include <sched.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mount.h>
#include <sys/syscall.h>
#include <unistd.h>
#include <linux/mount.h>

static int sys_fsopen(const char *n, unsigned f) { return syscall(SYS_fsopen, n, f); }
static int sys_fsconfig(int fd, unsigned cmd, const char *k, const void *v, int aux) {
	return syscall(SYS_fsconfig, fd, cmd, k, v, aux);
}
static int sys_fsmount(int fd, unsigned f, unsigned a) { return syscall(SYS_fsmount, fd, f, a); }
static int sys_move_mount(int fd, const char *fp, int tfd, const char *tp, unsigned f) {
	return syscall(SYS_move_mount, fd, fp, tfd, tp, f);
}
static int sys_open_tree(int dfd, const char *p, unsigned f) { return syscall(SYS_open_tree, dfd, p, f); }
static int sys_mount_setattr(int dfd, const char *p, unsigned f, struct mount_attr *a, size_t s) {
	return syscall(SYS_mount_setattr, dfd, p, f, a, s);
}

static void dump_log(int fs) {
	char buf[4096];
	ssize_t n;
	while ((n = read(fs, buf, sizeof(buf) - 1)) > 0) {
		buf[n] = 0;
		fprintf(stderr, "fslog: %s\n", buf);
	}
}

static void die(const char *what, int fs) {
	int e = errno;
	fprintf(stderr, "ERR %s: %s (errno=%d)\n", what, strerror(e), e);
	if (fs >= 0) dump_log(fs);
	exit(e ? e : 1);
}

static int userns_fd(const char *pid) {
	char p[64];
	snprintf(p, sizeof p, "/proc/%s/ns/user", pid);
	int fd = open(p, O_RDONLY | O_CLOEXEC);
	if (fd < 0) die("open userns", -1);
	return fd;
}

static int idmap_tree(const char *path, const char *pid) {
	int t = sys_open_tree(AT_FDCWD, path, OPEN_TREE_CLONE | OPEN_TREE_CLOEXEC);
	if (t < 0) die("open_tree", -1);
	struct mount_attr a = {.attr_set = MOUNT_ATTR_IDMAP, .userns_fd = userns_fd(pid)};
	if (sys_mount_setattr(t, "", AT_EMPTY_PATH, &a, sizeof a)) die("mount_setattr idmap layer", -1);
	return t;
}

static int layer_fd(const char *path, const char *idpid) {
	if (idpid) return idmap_tree(path, idpid);
	int fd = open(path, O_RDONLY | O_DIRECTORY | O_CLOEXEC);
	if (fd < 0) die("open layer", -1);
	return fd;
}

static void enter_mntns(const char *pid) {
	char p[64];
	snprintf(p, sizeof p, "/proc/%s/ns/mnt", pid);
	int fd = open(p, O_RDONLY | O_CLOEXEC);
	if (fd < 0) die("open mntns", -1);
	if (setns(fd, CLONE_NEWNS)) die("setns mnt", -1);
}

static void do_move(int mfd, const char *target, const char *nspid) {
	if (nspid) enter_mntns(nspid);
	if (sys_move_mount(mfd, "", AT_FDCWD, target, MOVE_MOUNT_F_EMPTY_PATH)) die("move_mount", -1);
}

int main(int argc, char **argv) {
	if (argc == 5 && !strcmp(argv[1], "bind-idmap")) {
		int t = idmap_tree(argv[2], argv[4]);
		do_move(t, argv[3], NULL);
		return 0;
	}
	const char *target = NULL, *nspid = NULL, *idpid = NULL, *layer_idpid = NULL;
	const char *opts[64], *lowers[1024], *upper = NULL, *work = NULL;
	int lower_fd[1024], nopts = 0, nl = 0, ro = 0, legacy = 0, upper_by_fd = 0;
	int c;
	while ((c = getopt(argc, argv, "l:L:u:w:B:o:rn:i:I:t:m")) != -1) {
		switch (c) {
		case 'l': lower_fd[nl] = 0; lowers[nl++] = optarg; break;
		case 'L': lower_fd[nl] = 1; lowers[nl++] = optarg; break;
		case 'u': upper = optarg; break;
		case 'w': work = optarg; break;
		case 'B': upper = optarg; upper_by_fd = 1; break;
		case 'o': opts[nopts++] = optarg; break;
		case 'r': ro = 1; break;
		case 'n': nspid = optarg; break;
		case 'i': idpid = optarg; break;
		case 'I': layer_idpid = optarg; break;
		case 't': target = optarg; break;
		case 'm': legacy = 1; break;
		default: fprintf(stderr, "bad args\n"); return 2;
		}
	}
	if (!target) { fprintf(stderr, "need -t\n"); return 2; }

	if (legacy) {
		static char data[1 << 20];
		size_t off = 0;
		off += snprintf(data + off, sizeof data - off, "lowerdir=");
		for (int i = 0; i < nl; i++)
			off += snprintf(data + off, sizeof data - off, "%s%s", i ? ":" : "", lowers[i]);
		if (upper) off += snprintf(data + off, sizeof data - off, ",upperdir=%s,workdir=%s", upper, work);
		for (int i = 0; i < nopts; i++) off += snprintf(data + off, sizeof data - off, ",%s", opts[i]);
		fprintf(stderr, "legacy option string length=%zu\n", off);
		if (mount("overlay", target, "overlay", ro ? MS_RDONLY : 0, data)) die("mount(2)", -1);
		return 0;
	}

	int fs = sys_fsopen("overlay", FSOPEN_CLOEXEC);
	if (fs < 0) die("fsopen", -1);
	for (int i = 0; i < nl; i++) {
		if (lower_fd[i]) {
			int fd = layer_fd(lowers[i], layer_idpid);
			if (sys_fsconfig(fs, FSCONFIG_SET_FD, "lowerdir+", NULL, fd)) die("fsconfig lowerdir+ fd", fs);
		} else if (sys_fsconfig(fs, FSCONFIG_SET_STRING, "lowerdir+", lowers[i], 0)) {
			die("fsconfig lowerdir+", fs);
		}
	}
	if (upper) {
		if (upper_by_fd) {
			// upper and work must share one mount, so idmap their common parent once.
			int b = layer_fd(upper, layer_idpid);
			int u = openat(b, "upper", O_RDONLY | O_DIRECTORY | O_CLOEXEC);
			int w = openat(b, "work", O_RDONLY | O_DIRECTORY | O_CLOEXEC);
			if (u < 0 || w < 0) die("openat upper/work", -1);
			if (sys_fsconfig(fs, FSCONFIG_SET_FD, "upperdir", NULL, u)) die("fsconfig upperdir fd", fs);
			if (sys_fsconfig(fs, FSCONFIG_SET_FD, "workdir", NULL, w)) die("fsconfig workdir fd", fs);
		} else {
			if (sys_fsconfig(fs, FSCONFIG_SET_STRING, "upperdir", upper, 0)) die("fsconfig upperdir", fs);
			if (sys_fsconfig(fs, FSCONFIG_SET_STRING, "workdir", work, 0)) die("fsconfig workdir", fs);
		}
	}
	for (int i = 0; i < nopts; i++) {
		char k[256];
		const char *eq = strchr(opts[i], '=');
		int r;
		if (eq) {
			snprintf(k, sizeof k, "%.*s", (int)(eq - opts[i]), opts[i]);
			r = sys_fsconfig(fs, FSCONFIG_SET_STRING, k, eq + 1, 0);
		} else {
			r = sys_fsconfig(fs, FSCONFIG_SET_FLAG, opts[i], NULL, 0);
		}
		if (r) die(opts[i], fs);
	}
	if (sys_fsconfig(fs, FSCONFIG_CMD_CREATE, NULL, NULL, 0)) die("fsconfig create", fs);
	dump_log(fs);
	int mfd = sys_fsmount(fs, FSMOUNT_CLOEXEC, ro ? MOUNT_ATTR_RDONLY : 0);
	if (mfd < 0) die("fsmount", fs);
	if (idpid) {
		struct mount_attr a = {.attr_set = MOUNT_ATTR_IDMAP, .userns_fd = userns_fd(idpid)};
		if (sys_mount_setattr(mfd, "", AT_EMPTY_PATH, &a, sizeof a)) die("mount_setattr idmap overlay", -1);
	}
	do_move(mfd, target, nspid);
	return 0;
}

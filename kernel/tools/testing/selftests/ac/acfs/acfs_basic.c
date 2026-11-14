// SPDX-License-Identifier: GPL-2.0
/*
 * acfs: files, directories, links, xattrs, truncate, persistence across
 * remounts, branches and snapshots, freeing a deleted branch, ENOSPC.
 * Needs an empty virtio disk at /dev/vda (vm.sh gives one).
 */
#include "acfs_util.h"

#ifndef FICLONE
#define FICLONE _IOW(0x94, 9, int)
#endif

#define MB	(1024 * 1024)

static const char *P(const char *m, const char *rel)
{
	static char paths[4][512];
	static int next;
	char *p = paths[next++ % 4];

	snprintf(p, sizeof(paths[0]), "%s/%s", m, rel);
	return p;
}

static int has_pattern(const char *p, uint64_t seed, size_t len)
{
	size_t got, i;
	char *buf = get_file(p, &got);
	int ok = buf && got == len;

	for (i = 0; ok && i < len; i++)
		ok = (uint8_t)buf[i] == pattern(seed, i);
	free(buf);
	return ok;
}

static int put_pattern(const char *p, uint64_t seed, size_t len)
{
	char *buf = malloc(len);
	size_t i;
	int ret;

	for (i = 0; i < len; i++)
		buf[i] = pattern(seed, i);
	ret = put_file(p, buf, len);
	free(buf);
	return ret;
}

static int same_file(const char *a, const char *b)
{
	size_t la, lb;
	char *x = get_file(a, &la), *y = get_file(b, &lb);
	int ok = x && y && la == lb && !memcmp(x, y, la);

	free(x);
	free(y);
	return ok;
}

static int count_entries(const char *dir)
{
	DIR *d = opendir(dir);
	struct dirent *e;
	int n = 0;

	if (!d)
		return -1;
	while ((e = readdir(d)))
		if (strcmp(e->d_name, ".") && strcmp(e->d_name, ".."))
			n++;
	closedir(d);
	return n;
}

static void populate(const char *m)
{
	char big[100];
	int fd;

	MUST(!put_file(P(m, "f"), "hello", 5), "f");
	MUST(!mkdir(P(m, "d"), 0755), "mkdir d");
	MUST(!put_pattern(P(m, "d/x"), 1, MB), "d/x");
	MUST(!symlink("d/x", P(m, "s")), "symlink");
	MUST(!link(P(m, "f"), P(m, "h")), "link");
	MUST(!mknod(P(m, "p"), S_IFIFO | 0600, 0), "mknod");
	MUST(!setxattr(P(m, "f"), "user.color", "blue", 4, 0), "setxattr");
	memset(big, 'x', sizeof(big));
	MUST(!setxattr(P(m, "f"), "user.n", big, sizeof(big), 0), "setxattr");
	CHECK(setxattr(P(m, "f"), "user.color", "red", 3, XATTR_CREATE) &&
	      errno == EEXIST, "XATTR_CREATE on existing");
	MUST(!put_pattern(P(m, "t"), 2, 10000), "t");
	MUST(!truncate(P(m, "t"), 5000), "truncate down");
	MUST(!truncate(P(m, "t"), 9000), "truncate up");
	MUST(!rename(P(m, "d/x"), P(m, "d/y")), "rename");
	MUST(!put_file(P(m, "e1"), "one", 3), "e1");
	MUST(!put_file(P(m, "e2"), "two", 3), "e2");
	{
		char a[512];

		snprintf(a, sizeof(a), "%s/e1", m);
		MUST(!renameat2(AT_FDCWD, a, AT_FDCWD, P(m, "e2"),
				RENAME_EXCHANGE), "exchange");
		snprintf(a, sizeof(a), "%s/e1", m);
		CHECK(renameat2(AT_FDCWD, a, AT_FDCWD, P(m, "f"),
				RENAME_NOREPLACE) && errno == EEXIST,
		      "noreplace");
	}
	MUST(!mkdir(P(m, "r"), 0755), "mkdir r");
	MUST(!mkdir(P(m, "r/sub"), 0755), "mkdir r/sub");
	CHECK(rmdir(P(m, "r")) && errno == ENOTEMPTY, "rmdir non-empty");
	MUST(!rmdir(P(m, "r/sub")), "rmdir r/sub");
	MUST(!rmdir(P(m, "r")), "rmdir r");

	fd = open(P(m, "u"), O_RDWR | O_CREAT, 0644);
	MUST(fd >= 0, "u");
	MUST(write(fd, "gone", 4) == 4, "write u");
	MUST(!unlink(P(m, "u")), "unlink u");
	{
		char b[8] = {};

		CHECK(pread(fd, b, 4, 0) == 4 && !memcmp(b, "gone", 4),
		      "read unlinked");
	}
	close(fd);

	fd = open(m, O_TMPFILE | O_RDWR, 0600);
	MUST(fd >= 0, "O_TMPFILE");
	MUST(write(fd, "tmp", 3) == 3, "write tmpfile");
	{
		char proc[64];

		snprintf(proc, sizeof(proc), "/proc/self/fd/%d", fd);
		MUST(!linkat(AT_FDCWD, proc, AT_FDCWD, P(m, "linked"),
			     AT_SYMLINK_FOLLOW), "linkat tmpfile");
	}
	close(fd);
}

static void check_tree(const char *m)
{
	struct stat a, b;
	char buf[512], *t;
	ssize_t n;
	size_t len, i;
	int ok;

	CHECK(file_is(P(m, "f"), "hello"), "%s f", m);
	CHECK(file_is(P(m, "h"), "hello"), "%s h", m);
	CHECK(!stat(P(m, "f"), &a) && a.st_nlink == 2, "%s nlink %d", m,
	      (int)a.st_nlink);
	CHECK(!stat(P(m, "h"), &b) && a.st_ino == b.st_ino, "%s link ino", m);
	CHECK(has_pattern(P(m, "d/y"), 1, MB), "%s d/y", m);
	CHECK(access(P(m, "d/x"), F_OK) && errno == ENOENT, "%s d/x gone", m);
	n = readlink(P(m, "s"), buf, sizeof(buf));
	CHECK(n == 3 && !memcmp(buf, "d/x", 3), "%s readlink", m);
	CHECK(!lstat(P(m, "p"), &a) && S_ISFIFO(a.st_mode), "%s fifo", m);
	n = getxattr(P(m, "f"), "user.color", buf, sizeof(buf));
	CHECK(n == 4 && !memcmp(buf, "blue", 4), "%s xattr", m);
	n = getxattr(P(m, "f"), "user.n", buf, sizeof(buf));
	CHECK(n == 100 && buf[99] == 'x', "%s xattr n", m);
	n = listxattr(P(m, "f"), buf, sizeof(buf));
	CHECK(n == (ssize_t)(sizeof("user.color") + sizeof("user.n")),
	      "%s listxattr %zd", m, n);
	t = get_file(P(m, "t"), &len);
	ok = t && len == 9000;
	for (i = 0; ok && i < len; i++)
		ok = (uint8_t)t[i] == (i < 5000 ? pattern(2, i) : 0);
	CHECK(ok, "%s truncated file", m);
	free(t);
	CHECK(file_is(P(m, "e1"), "two") && file_is(P(m, "e2"), "one"),
	      "%s exchange", m);
	CHECK(access(P(m, "r"), F_OK) && errno == ENOENT, "%s r gone", m);
	CHECK(access(P(m, "u"), F_OK) && errno == ENOENT, "%s u gone", m);
	CHECK(file_is(P(m, "linked"), "tmp"), "%s linked tmpfile", m);
	CHECK(!stat(P(m, "d"), &a) && a.st_nlink == 2, "%s dir nlink", m);
}

static void test_files(void)
{
	populate("/mnt/a");
	check_tree("/mnt/a");
	CHECK(count_entries("/mnt/a") == 9, "entries %d",
	      count_entries("/mnt/a"));
	do_umount("/mnt/a");
	do_mount("/mnt/a", NULL);
	check_tree("/mnt/a");
	drop_caches();
	check_tree("/mnt/a");
	printf("acfs: files ok\n");
}

static void test_big(const char *m)
{
	char *buf = malloc(MB);
	struct stat st;
	size_t i;
	int fd, ok = 1;

	fd = open(P(m, "big"), O_RDWR | O_CREAT | O_TRUNC, 0644);
	MUST(fd >= 0, "big");
	for (int c = 0; c < 64; c++) {
		for (i = 0; i < MB; i++)
			buf[i] = pattern(7, (uint64_t)c * MB + i);
		MUST(write(fd, buf, MB) == MB, "write big");
	}
	MUST(!fsync(fd), "fsync big");
	for (i = 0; i < MB; i++)
		buf[i] = pattern(8, i);
	MUST(pwrite(fd, buf, MB, 10 * MB + 123) == MB, "overwrite big");
	close(fd);
	drop_caches();
	fd = open(P(m, "big"), O_RDONLY);
	for (int c = 0; c < 64 && ok; c++) {
		MUST(read(fd, buf, MB) == MB, "read big");
		for (i = 0; i < MB && ok; i++) {
			uint64_t off = (uint64_t)c * MB + i;
			uint8_t want = off >= 10 * MB + 123 &&
				       off < 11 * MB + 123 ?
				       pattern(8, off - 10 * MB - 123) :
				       pattern(7, off);

			ok = (uint8_t)buf[i] == want;
		}
	}
	close(fd);
	CHECK(ok, "big file content");
	CHECK(!stat(P(m, "big"), &st) && st.st_blocks >= 64 * MB / 512 &&
	      st.st_blocks < 66 * MB / 512, "big st_blocks %lld",
	      (long long)st.st_blocks);

	fd = open(P(m, "sparse"), O_RDWR | O_CREAT | O_TRUNC, 0644);
	MUST(pwrite(fd, "end", 3, 8ULL << 30) == 3, "sparse write");
	MUST(!fsync(fd), "sparse fsync");
	MUST(pread(fd, buf, 4096, 1ULL << 30) == 4096, "sparse read");
	for (i = 0; i < 4096 && ok; i++)
		ok = !buf[i];
	CHECK(ok, "hole reads zero");
	CHECK(!fstat(fd, &st) && st.st_size == (8LL << 30) + 3 &&
	      st.st_blocks == 8, "sparse stat %lld %lld",
	      (long long)st.st_size, (long long)st.st_blocks);
	close(fd);
	MUST(!unlink(P(m, "sparse")), "unlink sparse");
	free(buf);
	printf("acfs: big files ok\n");
}

static void test_many(const char *m)
{
	char name[64];
	int i, ok = 1;

	MUST(!mkdir(P(m, "many"), 0755), "mkdir many");
	for (i = 0; i < 3000; i++) {
		snprintf(name, sizeof(name), "many/file-%05d", i);
		MUST(!put_file(P(m, name), name, strlen(name)), "%s", name);
	}
	CHECK(count_entries(P(m, "many")) == 3000, "3000 entries");
	for (i = 0; i < 3000; i += 2) {
		snprintf(name, sizeof(name), "many/file-%05d", i);
		MUST(!unlink(P(m, name)), "unlink %s", name);
	}
	CHECK(count_entries(P(m, "many")) == 1500, "1500 entries");
	do_umount(m);
	do_mount(m, NULL);
	CHECK(count_entries(P(m, "many")) == 1500, "1500 after remount");
	for (i = 1; i < 3000 && ok; i += 2) {
		snprintf(name, sizeof(name), "many/file-%05d", i);
		ok = file_is(P(m, name), name);
	}
	CHECK(ok, "many contents");
	printf("acfs: directories ok\n");
}

static void test_branches(void)
{
	struct acfs_branch_info snap, b1, info;
	struct stat sa, sb;
	uint64_t before, after;
	int fd;

	MUST(!branch_create("/mnt/a", 0, "snap", ACFS_BRANCH_RDONLY, &snap),
	     "create snap");
	MUST(!branch_create("/mnt/a", 0, "b1", 0, &b1), "create b1");
	CHECK(branch_create("/mnt/a", 0, "b1", 0, NULL) && errno == EEXIST,
	      "duplicate name");
	CHECK(snap.flags == ACFS_BRANCH_RDONLY && snap.parent == 1 &&
	      b1.id > snap.id, "branch info");
	CHECK(!branch_get("/mnt/a", 0, "b1", &info) && info.id == b1.id,
	      "get by name");

	do_mount("/mnt/s", "branch=snap");
	do_mount("/mnt/b", "branch=b1");
	check_tree("/mnt/s");
	check_tree("/mnt/b");
	fd = open("/mnt/s/new", O_WRONLY | O_CREAT, 0644);
	CHECK(fd < 0 && errno == EROFS, "snapshot is read-only");
	CHECK(unlink("/mnt/s/f") && errno == EROFS, "snapshot unlink");

	MUST(!put_file("/mnt/b/f", "branch", 6), "write b1 f");
	MUST(!put_file("/mnt/a/f", "main1", 5), "write main f");
	CHECK(file_is("/mnt/a/f", "main1") && file_is("/mnt/a/h", "main1"),
	      "main sees its write");
	CHECK(file_is("/mnt/b/f", "branch") && file_is("/mnt/b/h", "branch"),
	      "b1 sees its write");
	CHECK(file_is("/mnt/s/f", "hello"), "snapshot unchanged");
	CHECK(!stat("/mnt/a/f", &sa) && !stat("/mnt/b/f", &sb) &&
	      sa.st_ino == sb.st_ino && sa.st_dev != sb.st_dev,
	      "same file, other branch");
	MUST(!put_file("/mnt/a/nm", "m", 1), "nm");
	MUST(!put_file("/mnt/b/nb", "b", 1), "nb");
	CHECK(!stat("/mnt/a/nm", &sa) && !stat("/mnt/b/nb", &sb) &&
	      sa.st_ino != sb.st_ino, "new files never share an ino");
	CHECK(access("/mnt/a/nb", F_OK) && access("/mnt/b/nm", F_OK),
	      "new files stay in their branch");

	{
		uint64_t f0 = free_blocks("/mnt/a");
		int in = open("/mnt/a/big", O_RDONLY);
		int out = open("/mnt/b/bigcopy", O_WRONLY | O_CREAT, 0644);
		loff_t len = 64 * MB;
		ssize_t n = 0, got;

		MUST(in >= 0 && out >= 0, "open for copy_file_range");
		while (n < len && (got = copy_file_range(in, NULL, out, NULL,
							 len - n, 0)) > 0)
			n += got;
		CHECK(n == len, "copy_file_range across branches: %zd", n);
		close(in);
		close(out);
		CHECK(free_blocks("/mnt/a") + 200 > f0,
		      "reflink shares blocks: %llu -> %llu",
		      (unsigned long long)f0,
		      (unsigned long long)free_blocks("/mnt/a"));
		drop_caches();
		CHECK(same_file("/mnt/a/big", "/mnt/b/bigcopy"), "copy content");
		out = open("/mnt/b/bigcopy", O_WRONLY);
		MUST(pwrite(out, "changed", 7, 5 * MB) == 7, "write copy");
		close(out);
		sync();
		drop_caches();
		CHECK(!same_file("/mnt/a/big", "/mnt/b/bigcopy"),
		      "copy diverged");
		in = open("/mnt/a/big", O_RDONLY);
		out = open("/mnt/a/big2", O_WRONLY | O_CREAT, 0644);
		CHECK(!ioctl(out, FICLONE, in), "FICLONE");
		close(in);
		close(out);
		CHECK(same_file("/mnt/a/big", "/mnt/a/big2"), "FICLONE content");
		MUST(!unlink("/mnt/a/big2"), "unlink big2");
	}
	MUST(!put_pattern("/mnt/b/big", 9, 32 * MB), "b1 big");
	sync();
	do_umount("/mnt/b");
	before = free_blocks("/mnt/a");
	MUST(!branch_delete("/mnt/a", b1.id), "delete b1");
	CHECK(branch_delete("/mnt/a", 1) && errno == EBUSY, "delete main");
	after = free_blocks("/mnt/a");
	CHECK(after >= before + 8192, "delete frees blocks: %llu -> %llu",
	      (unsigned long long)before, (unsigned long long)after);
	CHECK(branch_get("/mnt/a", 0, "b1", &info) && errno == ENOENT,
	      "b1 gone");
	CHECK(mount(DEV, "/mnt/b", "acfs", 0, "branch=b1") && errno == EINVAL,
	      "mount deleted branch");

	do_umount("/mnt/s");
	do_umount("/mnt/a");
	do_mount("/mnt/a", NULL);
	CHECK(!branch_get("/mnt/a", 0, "snap", &info) && info.id == snap.id,
	      "snap after remount");
	CHECK(branch_get("/mnt/a", 0, "b1", &info) && errno == ENOENT,
	      "b1 gone after remount");
	do_mount("/mnt/s", "branch=snap");
	check_tree("/mnt/s");
	do_umount("/mnt/s");
	printf("acfs: branches ok\n");
}

static int diff_all(uint64_t a, uint64_t b, struct acfs_diff_entry *out,
		    int max, int step)
{
	struct acfs_diff d = { .size = sizeof(d), .a = a, .b = b };
	int n = 0, fd = open("/mnt/a", O_RDONLY | O_DIRECTORY);

	do {
		d.count = step;
		d.entries = (uintptr_t)(out + n);
		if (ioctl(fd, ACFS_IOC_DIFF, &d)) {
			n = -1;
			break;
		}
		n += d.count;
	} while (!d.done && n + step <= max);
	close(fd);
	return n;
}

static int touched(struct acfs_diff_entry *e, int n, uint64_t ino)
{
	for (int i = 0; i < n; i++)
		if (e[i].objectid == ino)
			return 1;
	return 0;
}

static void test_diff(void)
{
	struct acfs_branch_info s1, s2;
	struct acfs_diff_entry e[256], e1[256];
	struct stat t, gone, big, root, nf;
	int n, n1, i, others = 0;

	MUST(!stat("/mnt/a/t", &t) && !stat("/mnt/a/e1", &gone) &&
	     !stat("/mnt/a/big", &big) && !stat("/mnt/a", &root), "stat");
	MUST(!branch_create("/mnt/a", 0, "d1", ACFS_BRANCH_RDONLY, &s1), "d1");
	MUST(!put_file("/mnt/a/f2", "new", 3), "f2");
	MUST(!truncate("/mnt/a/t", 100), "truncate t");
	MUST(!unlink("/mnt/a/e1"), "unlink e1");
	MUST(!branch_create("/mnt/a", 0, "d2", ACFS_BRANCH_RDONLY, &s2), "d2");
	MUST(!stat("/mnt/a/f2", &nf), "stat f2");

	CHECK(diff_all(s1.id, s1.id, e, 256, 256) == 0, "no diff to itself");
	n = diff_all(s1.id, s2.id, e, 256, 256);
	CHECK(n > 0 && n < 40, "diff has %d entries", n);
	CHECK(touched(e, n, t.st_ino) && touched(e, n, gone.st_ino) &&
	      touched(e, n, nf.st_ino) && touched(e, n, root.st_ino),
	      "diff names the changed inodes");
	CHECK(!touched(e, n, big.st_ino), "diff leaves big out");
	for (i = 0; i < n; i++)
		others += e[i].objectid != t.st_ino &&
			  e[i].objectid != gone.st_ino &&
			  e[i].objectid != nf.st_ino &&
			  e[i].objectid != root.st_ino &&
			  e[i].objectid != ACFS_ORPHAN_OBJID;
	CHECK(!others, "diff names %d other inodes", others);
	for (i = 0; i < n; i++)
		if (e[i].objectid == gone.st_ino)
			CHECK(e[i].change == ACFS_DIFF_REMOVED, "e1 removed");
	n1 = diff_all(s1.id, s2.id, e1, 256, 1);
	CHECK(n1 == n && !memcmp(e, e1, n * sizeof(e[0])),
	      "diff one at a time: %d of %d", n1, n);
	MUST(!branch_delete("/mnt/a", s1.id) && !branch_delete("/mnt/a", s2.id),
	     "delete d1 d2");
	printf("acfs: diff ok\n");
}

static void test_enospc(const char *m)
{
	char *buf = calloc(1, MB);
	ssize_t n = 0;
	int fd, err = 0;

	fd = open(P(m, "fill"), O_WRONLY | O_CREAT | O_TRUNC, 0644);
	MUST(fd >= 0, "fill");
	for (;;) {
		n = write(fd, buf, MB);
		if (n < 0) {
			err = errno;
			break;
		}
		if (fsync(fd)) {
			err = errno;
			break;
		}
	}
	close(fd);
	CHECK(err == ENOSPC, "write until full: errno %d", err);
	MUST(!unlink(P(m, "fill")), "unlink fill");
	sync();
	MUST(!put_file(P(m, "after"), "ok", 2), "write after ENOSPC");
	do_umount(m);
	do_mount(m, NULL);
	CHECK(file_is(P(m, "after"), "ok"), "after");
	free(buf);
	printf("acfs: ENOSPC ok\n");
}

int main(void)
{
	struct statfs s;

	mkfs();
	do_mount("/mnt/a", NULL);
	MUST(!statfs("/mnt/a", &s) && s.f_type == ACFS_SUPER_MAGIC, "statfs");
	test_files();
	test_big("/mnt/a");
	test_many("/mnt/a");
	test_branches();
	test_diff();
	test_enospc("/mnt/a");
	do_umount("/mnt/a");
	run("/t/fsck.acfs", DEV, NULL);
	printf("acfs: %d failures\n", failures);
	return failures ? 1 : 0;
}

// SPDX-License-Identifier: GPL-2.0
/*
 * acfs across a crash, in two boots on the same disk (vm.sh runs
 * "acfs_crash 1", then "acfs_crash 2").
 *
 * 1: while a child churns (random operations, branches made and deleted),
 *    write checkpoint files c0, c1, ... and fsync each, recording the last
 *    fsynced one on /dev/vdb; after a few seconds reboot at once (sysrq b),
 *    so the disk has whatever writeback got out of the last transactions.
 * 2: fsck.acfs must find the last commit clean, it must mount, and every
 *    recorded checkpoint must be there; then unmount and fsck again.
 */
#include "acfs_ops.h"

#include <sys/random.h>
#include <time.h>

#define LOG	"/dev/vdb"

static void record(int i)
{
	static char buf[4096] __attribute__((aligned(4096)));
	int fd = open(LOG, O_WRONLY | O_DIRECT | O_SYNC);

	MUST(fd >= 0, "open " LOG);
	memset(buf, 0, sizeof(buf));
	snprintf(buf, sizeof(buf), "acfs-crash %d\n", i);
	MUST(pwrite(fd, buf, sizeof(buf), 0) == sizeof(buf), "record");
	close(fd);
}

static int recorded(void)
{
	char buf[64] = {};
	int fd = open(LOG, O_RDONLY), i = -1;

	if (fd >= 0 && read(fd, buf, sizeof(buf) - 1) > 0)
		sscanf(buf, "acfs-crash %d", &i);
	if (fd >= 0)
		close(fd);
	return i;
}

static void churn(void)
{
	struct op o;
	int n = 0;

	MUST(!mkdir("/mnt/a/churn", 0755), "churn");
	for (;;) {
		nents = 0;
		scan("/mnt/a/churn", "");
		random_op(&o);
		apply("/mnt/a/churn", &o);
		if (!(++n % 200)) {
			struct acfs_branch_info info;
			char name[32];

			snprintf(name, sizeof(name), "x%d", n);
			if (!branch_create("/mnt/a", 0, name, below(2) ?
					   ACFS_BRANCH_RDONLY : 0, &info) &&
			    below(2))
				branch_delete("/mnt/a", info.id);
		}
	}
}

static size_t cp_size(int i)
{
	return 1000 + (i * 7919) % 200000;
}

static void phase1(void)
{
	struct timespec start, now;
	uint32_t ms;
	int i, fd;

	mkfs();
	record(-1);
	do_mount("/mnt/a", NULL);
	if (getrandom(&rng, sizeof(rng), 0) != sizeof(rng) || !rng)
		rng = time(NULL) | 1;
	ms = 1500 + below(4000);
	if (!fork())
		churn();
	clock_gettime(CLOCK_MONOTONIC, &start);
	for (i = 0;; i++) {
		char p[64], *buf;
		size_t len = cp_size(i);

		snprintf(p, sizeof(p), "/mnt/a/c%d", i);
		buf = malloc(len);
		for (size_t j = 0; j < len; j++)
			buf[j] = pattern(i, j);
		fd = open(p, O_WRONLY | O_CREAT | O_TRUNC, 0644);
		MUST(fd >= 0 && write(fd, buf, len) == (ssize_t)len &&
		     !fsync(fd), "checkpoint %d", i);
		close(fd);
		free(buf);
		record(i);
		clock_gettime(CLOCK_MONOTONIC, &now);
		if ((now.tv_sec - start.tv_sec) * 1000 +
		    (now.tv_nsec - start.tv_nsec) / 1000000 > ms)
			break;
	}
	printf("acfs: crashing after %u ms, checkpoint %d\n", ms, i);
	fflush(stdout);
	fd = open("/proc/sysrq-trigger", O_WRONLY);
	MUST(fd >= 0 && write(fd, "b", 1) == 1, "sysrq");
	pause();
}

static void phase2(void)
{
	int last = recorded(), i, ok = 1;

	run("/t/fsck.acfs", DEV, NULL);
	do_mount("/mnt/a", NULL);
	for (i = 0; i <= last && ok; i++) {
		char p[64], *buf;
		size_t len = 0;

		snprintf(p, sizeof(p), "/mnt/a/c%d", i);
		buf = get_file(p, &len);
		ok = buf && len == cp_size(i);
		for (size_t j = 0; ok && j < len; j++)
			ok = (uint8_t)buf[j] == pattern(i, j);
		free(buf);
	}
	CHECK(ok, "checkpoint %d of %d", i - 1, last);
	digest("/mnt/a", "", 0);
	do_umount("/mnt/a");
	run("/t/fsck.acfs", DEV, NULL);
	printf("acfs: %d checkpoints survived the crash; %d failures\n",
	       last + 1, failures);
	exit(failures ? 1 : 0);
}

int main(int argc, char **argv)
{
	if (argc > 1 && !strcmp(argv[1], "1"))
		phase1();
	if (argc > 1 && !strcmp(argv[1], "2"))
		phase2();
	fprintf(stderr, "usage: acfs_crash 1|2 (see vm.sh)\n");
	return 2;
}

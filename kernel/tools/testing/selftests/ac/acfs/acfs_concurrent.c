// SPDX-License-Identifier: GPL-2.0
/*
 * acfs under concurrency: worker processes run random operations, each in
 * its own directory mirrored on tmpfs, while the main process takes,
 * mounts, reads and deletes snapshots, syncs and drops caches. Then every
 * directory must still match its mirror, across unmount, fsck.acfs and
 * mount. ACFS_OPS (3000) operations per worker.
 */
#include "acfs_ops.h"

#define WORKERS	6
#define KEEP	4

static void worker(int k, int ops)
{
	char a[64], r[64];

	rng = (k + 1) * 0x9e3779b97f4a7c15ULL;
	snprintf(a, sizeof(a), "/mnt/a/w%d", k);
	snprintf(r, sizeof(r), "/tmp/r/w%d", k);
	MUST(!mkdir(a, 0755) && !mkdir(r, 0755), "worker roots");
	run_ops(a, r, ops, 0);
	compare(a, r, "at the end of a worker");
	exit(failures ? 1 : 0);
}

int main(void)
{
	int ops = getenv("ACFS_OPS") ? atoi(getenv("ACFS_OPS")) : 3000;
	uint64_t ids[KEEP] = {};
	int live = WORKERS, round = 0, snaps = 0, k, status;
	char a[64], r[64], name[64];
	pid_t pid;

	mkfs();
	do_mount("/mnt/a", NULL);
	MUST(!mkdir("/tmp/r", 0755), "/tmp/r");
	for (k = 0; k < WORKERS; k++)
		if (!fork())
			worker(k, ops);
	while (live) {
		struct acfs_branch_info info;

		pid = waitpid(-1, &status, WNOHANG);
		if (pid > 0) {
			live--;
			CHECK(WIFEXITED(status) && !WEXITSTATUS(status),
			      "worker %d failed", pid);
			continue;
		}
		switch (round++ % 5) {
		case 0:
			snprintf(name, sizeof(name), "c%d", snaps);
			if (ids[snaps % KEEP])
				MUST(!branch_delete("/mnt/a", ids[snaps % KEEP]),
				     "delete snapshot");
			MUST(!branch_create("/mnt/a", 0, name,
					    ACFS_BRANCH_RDONLY, &info),
			     "snapshot");
			ids[snaps++ % KEEP] = info.id;
			break;
		case 1:
			snprintf(name, sizeof(name), "branchid=%llu",
				 (unsigned long long)ids[(snaps - 1) % KEEP]);
			do_mount("/mnt/s", name);
			digest("/mnt/s", "", 0);
			do_umount("/mnt/s");
			break;
		case 2:
			sync();
			break;
		case 3:
			drop_caches();
			break;
		default:
			usleep(20000);
		}
	}
	do_umount("/mnt/a");
	run("/t/fsck.acfs", DEV, NULL);
	do_mount("/mnt/a", NULL);
	for (k = 0; k < WORKERS; k++) {
		snprintf(a, sizeof(a), "/mnt/a/w%d", k);
		snprintf(r, sizeof(r), "/tmp/r/w%d", k);
		compare(a, r, "after remount");
	}
	do_umount("/mnt/a");
	run("/t/fsck.acfs", DEV, NULL);
	printf("acfs: %d workers x %d ops, %d snapshots; %d failures\n",
	       WORKERS, ops, snaps, failures);
	return failures ? 1 : 0;
}

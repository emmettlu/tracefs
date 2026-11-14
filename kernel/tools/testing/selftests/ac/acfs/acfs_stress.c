// SPDX-License-Identifier: GPL-2.0
/*
 * acfs random operations, mirrored on tmpfs: every operation runs on both
 * trees and must give the same result; the trees are compared every so
 * often, and across unmount, fsck.acfs and mount. Read-only snapshots are
 * taken along the way and checked against a digest of the tmpfs tree at
 * that moment; writable branches get their own random operations against
 * a copy, while main must not change. Seeds 1..ACFS_SEEDS (3), each
 * ACFS_OPS (6000) operations, on a fresh file system.
 */
#include "acfs_ops.h"

#define A	"/mnt/a/w"
#define R	"/tmp/r/w"
#define NSNAP	64
static void fsck(void)
{
	run("/t/fsck.acfs", DEV, NULL);
}

static void remount(void)
{
	do_umount("/mnt/a");
	fsck();
	do_mount("/mnt/a", NULL);
}

static void one_seed(uint64_t seed, int ops)
{
	struct snap {
		uint64_t id, digest;
	} snaps[NSNAP];
	int nsnap = 0, i;
	char name[64];

	rng = seed * 0x9e3779b97f4a7c15ULL + 1;
	seq = 0;
	mkfs();
	do_mount("/mnt/a", NULL);
	run("/bin/busybox", "rm", "-rf", "/tmp/r", NULL);
	MUST(!mkdir("/tmp/r", 0755) && !mkdir(R, 0755) && !mkdir(A, 0755),
	     "roots");

	for (i = 0; i < ops; i += 500) {
		run_ops(A, R, 500, 0);
		compare(A, R, "after a batch");
		switch (below(4)) {
		case 0:
			remount();
			compare(A, R, "after remount");
			break;
		case 1:
			if (nsnap < NSNAP) {
				struct acfs_branch_info info;

				snprintf(name, sizeof(name), "s%d", nsnap);
				MUST(!branch_create("/mnt/a", 0, name,
						    ACFS_BRANCH_RDONLY, &info),
				     "snapshot");
				snaps[nsnap].id = info.id;
				snaps[nsnap++].digest =
					digest(R, "", 0xcbf29ce484222325ULL);
			}
			break;
		case 2: {
			struct acfs_branch_info info;
			uint64_t before = digest(R, "", 0xcbf29ce484222325ULL);

			snprintf(name, sizeof(name), "b%d", i);
			MUST(!branch_create("/mnt/a", 0, name, 0, &info),
			     "branch");
			snprintf(name, sizeof(name), "branch=b%d", i);
			do_mount("/mnt/b", name);
			run("/bin/busybox", "rm", "-rf", "/tmp/rb", NULL);
			mkdir("/tmp/rb", 0755);
			copy_tree(R, "/tmp/rb/w");
			compare("/mnt/b/w", "/tmp/rb/w", "in a new branch");
			run_ops("/mnt/b/w", "/tmp/rb/w", 300, 0);
			compare("/mnt/b/w", "/tmp/rb/w", "branch ops");
			compare(A, R, "main during branch ops");
			MUST(digest(A, "", 0xcbf29ce484222325ULL) == before,
			     "main changed");
			do_umount("/mnt/b");
			if (below(2))
				MUST(!branch_delete("/mnt/a", info.id),
				     "delete branch");
			break;
		}
		default:
			drop_caches();
			compare(A, R, "after dropping caches");
		}
	}
	remount();
	compare(A, R, "at the end");
	for (i = 0; i < nsnap; i++) {
		snprintf(name, sizeof(name), "branchid=%llu",
			 (unsigned long long)snaps[i].id);
		do_mount("/mnt/s", name);
		MUST(digest("/mnt/s/w", "", 0xcbf29ce484222325ULL) ==
		     snaps[i].digest, "snapshot %d differs", i);
		do_umount("/mnt/s");
	}
	for (i = 0; i < nsnap; i += 2)
		MUST(!branch_delete("/mnt/a", snaps[i].id), "delete snapshot");
	remount();
	compare(A, R, "after deleting snapshots");
	do_umount("/mnt/a");
	fsck();
	printf("acfs: seed %llu: %d ops, %d snapshots ok\n",
	       (unsigned long long)seed, ops, nsnap);
}

int main(void)
{
	int seeds = getenv("ACFS_SEEDS") ? atoi(getenv("ACFS_SEEDS")) : 3;
	int ops = getenv("ACFS_OPS") ? atoi(getenv("ACFS_OPS")) : 6000;

	for (int s = 1; s <= seeds; s++)
		one_seed(s, ops);
	printf("acfs: %d failures\n", failures);
	return failures ? 1 : 0;
}

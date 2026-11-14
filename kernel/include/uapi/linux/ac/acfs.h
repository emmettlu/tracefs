/* SPDX-License-Identifier: GPL-2.0 WITH Linux-syscall-note */
/*
 * acfs: the Agent Computer File System.
 *
 * Documentation/userspace-api/ac/acfs.rst describes the interface; this header
 * is its ABI. Structures only grow at their end (with a size field),
 * numbers are never reused. The ioctls work on any fd of an acfs file
 * system.
 */
#ifndef _UAPI_LINUX_AC_ACFS_H
#define _UAPI_LINUX_AC_ACFS_H

#include <linux/ioctl.h>
#include <linux/types.h>

#define ACFS_IOC_MAGIC		0xAC

#define ACFS_BRANCH_RDONLY	(1ULL << 0)	/* a snapshot: never changes */
#define ACFS_BRANCH_DELETED	(1ULL << 1)	/* out only: being dropped */

struct acfs_branch_info {
	__u32 size;		/* sizeof(struct acfs_branch_info) */
	__u32 reserved0;
	__u64 id;
	__u64 parent;		/* branch it was made from, 0: none */
	__u64 flags;		/* ACFS_BRANCH_* */
	__u64 gen;		/* transaction that made it */
	__s64 ctime_sec;
	__u32 ctime_nsec;
	__u32 reserved1;
	char name[64];		/* NUL-terminated */
};

/*
 * ACFS_IOC_BRANCH_CREATE: make branch name from branch src (0: the fd's
 * branch) as it is now, after writing back everything. O(1): the new
 * branch shares all blocks until either side changes them. flags:
 * ACFS_BRANCH_RDONLY for a snapshot. Returns the new branch in info.
 * Needs CAP_SYS_ADMIN or ownership of the mount's root directory.
 */
struct acfs_branch_create {
	__u32 size;		/* sizeof(struct acfs_branch_create) */
	__u32 reserved0;
	__u64 src;
	__u64 flags;
	char name[64];
	struct acfs_branch_info info;	/* out */
};

/*
 * ACFS_IOC_BRANCH_DELETE: delete branch id (not ACFS_MAIN_BRANCH). It
 * disappears at once; its blocks are freed when nothing uses it any
 * more (mounts, open files, cached inodes). Needs CAP_SYS_ADMIN or
 * ownership of the mount's root directory.
 */
struct acfs_branch_delete {
	__u32 size;		/* sizeof(struct acfs_branch_delete) */
	__u32 reserved0;
	__u64 id;
};

/*
 * ACFS_IOC_BRANCH_GET: the branch with id, or else with name, or else
 * the fd's branch (id 0, name "").
 */
struct acfs_branch_get {
	__u32 size;		/* sizeof(struct acfs_branch_get) */
	__u32 reserved0;
	__u64 id;
	char name[64];
	struct acfs_branch_info info;	/* out */
};

/*
 * ACFS_IOC_BRANCH_LIST: up to count branches with id >= start, in id
 * order, into the array at ptr. Returns how many in count; start for
 * the next call is the last id + 1.
 */
struct acfs_branch_list {
	__u32 size;		/* sizeof(struct acfs_branch_list) */
	__u32 count;
	__u64 start;
	__u64 ptr;		/* struct acfs_branch_info[count] */
};

/*
 * ACFS_IOC_DIFF: the items that differ between branches a and b, in key
 * order, after the key pos (all zero to start). Only the parts of the two
 * trees that are not shared are read, so the cost follows the size of the
 * difference. Up to count entries go to the array at entries; count says
 * how many, pos is the last one's key, and done is set when there are no
 * more. Item types and keys are those of acfs_tree.h: an inode changed if
 * any item with its number as objectid did; ACFS_DIRENT_KEY entries say
 * which names of a directory changed (by hash), ACFS_EXTENT_KEY which
 * blocks of a file. Needs CAP_SYS_ADMIN or ownership of the mount's
 * root directory.
 */
#define ACFS_DIFF_ADDED		1	/* only in b */
#define ACFS_DIFF_REMOVED	2	/* only in a */
#define ACFS_DIFF_CHANGED	3	/* in both, different */

struct acfs_diff_entry {
	__u64 objectid;
	__u64 offset;
	__u8 type;
	__u8 change;		/* ACFS_DIFF_* */
	__u8 reserved[6];
};

struct acfs_diff {
	__u32 size;		/* sizeof(struct acfs_diff) */
	__u32 count;		/* in: room at entries; out: filled */
	__u64 a;
	__u64 b;
	__u64 entries;		/* struct acfs_diff_entry[count] */
	__u64 pos_objectid;	/* in/out */
	__u64 pos_offset;
	__u8 pos_type;
	__u8 done;		/* out */
	__u8 reserved[6];
};

/*
 * ACFS_IOC_BRANCH_OPEN: a new fd (the return value) of the root directory
 * of the branch with id, or else with name, on the fd's mount: read its
 * files with openat() and friends without mounting it. flags may be
 * O_CLOEXEC. Needs CAP_SYS_ADMIN or ownership of the mount's root
 * directory.
 */
struct acfs_branch_open {
	__u32 size;		/* sizeof(struct acfs_branch_open) */
	__u32 flags;
	__u64 id;
	char name[64];
};

#define ACFS_IOC_BRANCH_CREATE	_IOWR(ACFS_IOC_MAGIC, 0x40, struct acfs_branch_create)
#define ACFS_IOC_BRANCH_DELETE	_IOW(ACFS_IOC_MAGIC, 0x41, struct acfs_branch_delete)
#define ACFS_IOC_BRANCH_GET	_IOWR(ACFS_IOC_MAGIC, 0x42, struct acfs_branch_get)
#define ACFS_IOC_BRANCH_LIST	_IOWR(ACFS_IOC_MAGIC, 0x43, struct acfs_branch_list)
#define ACFS_IOC_DIFF		_IOWR(ACFS_IOC_MAGIC, 0x44, struct acfs_diff)
#define ACFS_IOC_BRANCH_OPEN	_IOW(ACFS_IOC_MAGIC, 0x45, struct acfs_branch_open)

#endif /* _UAPI_LINUX_AC_ACFS_H */

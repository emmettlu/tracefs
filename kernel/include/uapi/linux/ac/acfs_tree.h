/* SPDX-License-Identifier: GPL-2.0 WITH Linux-syscall-note */
/*
 * acfs on-disk format.
 *
 * Documentation/userspace-api/ac/acfs.rst describes it. Everything is little
 * endian, blocks are 4 KiB. Blocks 0 and 1 hold the two superblock slots,
 * the refcount table follows, then tree nodes and file data.
 */
#ifndef _UAPI_LINUX_AC_ACFS_TREE_H
#define _UAPI_LINUX_AC_ACFS_TREE_H

#include <linux/types.h>

#define ACFS_SUPER_MAGIC	0x41434653	/* statfs f_type, "ACFS" */
#define ACFS_NODE_MAGIC		0x41434e44
#define ACFS_RC_MAGIC		0x41435243
#define ACFS_VERSION		1

#define ACFS_BLOCK_SHIFT	12
#define ACFS_BLOCK_SIZE		(1U << ACFS_BLOCK_SHIFT)
#define ACFS_SB_SLOTS		2	/* blocks 0 and 1, slot = gen % 2 */
#define ACFS_RC_START		2	/* first block of the refcount table */
#define ACFS_RC_PER_PAGE	1016	/* refcounts in one table page */

#define ACFS_ROOT_INO		256	/* root directory of every branch */
#define ACFS_FIRST_INO		257
#define ACFS_MAIN_BRANCH	1	/* made by mkfs, mounted by default */
#define ACFS_ORPHAN_OBJID	((__u64)-5)

#define ACFS_NAME_LEN		255
#define ACFS_BRANCH_NAME_LEN	63
#define ACFS_MAX_LEVEL		8
#define ACFS_EXTENT_MAX		128	/* blocks in one extent item */
#define ACFS_ITEM_MAX		2000	/* bytes of one item's data */

/* Item types, the second part of a key. */
#define ACFS_INODE_KEY		1	/* (ino, 1, 0): acfs_inode_item */
#define ACFS_INLINE_KEY		2	/* (ino, 2, 0): symlink target */
#define ACFS_XATTR_KEY		3	/* (ino, 3, hash): acfs_xattr... */
#define ACFS_DIRENT_KEY		4	/* (dir, 4, hash): acfs_dirent... */
#define ACFS_DIRINDEX_KEY	5	/* (dir, 5, index): one acfs_dirent */
#define ACFS_EXTENT_KEY		6	/* (ino, 6, last block): acfs_extent */
#define ACFS_ORPHAN_KEY		7	/* (ACFS_ORPHAN_OBJID, 7, ino) */
#define ACFS_BRANCH_KEY		16	/* root tree: (id, 16, 0) */

struct acfs_disk_key {
	__le64 objectid;
	__u8 type;
	__le64 offset;
} __attribute__((__packed__));

/* crc32c covers each block from byte 4 to its end. */
struct acfs_super {
	__le32 csum;
	__le32 magic;
	__le32 version;
	__le32 block_size;
	__le64 gen;		/* transaction that wrote this slot */
	__le64 nr_blocks;
	__le64 rc_pages;	/* table: 2 * rc_pages blocks from RC_START */
	__le64 root_tree;	/* block of the root tree's root node */
	__le64 next_ino;
	__le64 next_branch;
	__le64 free_blocks;
	__le64 flags;
	__u8 uuid[16];
	__le64 reserved[16];
};

/*
 * Page p of the refcount table lives in two slots, blocks
 * RC_START + 2p and RC_START + 2p + 1. The valid one with the highest
 * gen not above the superblock's is current; a commit writes the other.
 */
struct acfs_rc_page {
	__le32 csum;
	__le32 magic;
	__le64 gen;
	__le64 page;
	__le64 reserved;
	__le32 count[ACFS_RC_PER_PAGE];
};

struct acfs_node_header {
	__le32 csum;
	__le32 magic;
	__le64 bnr;		/* where the node lives */
	__le64 gen;		/* transaction that wrote it */
	__le64 owner;		/* branch that wrote it, 0: root tree */
	__le16 nritems;
	__u8 level;		/* 0: leaf */
	__u8 reserved[5];
};

/* Internal node: an array of these after the header. */
struct acfs_key_ptr {
	struct acfs_disk_key key;
	__le64 bnr;
} __attribute__((__packed__));

/*
 * Leaf: an array of these after the header; item i's data is at
 * [offset, offset + size) of the block, packed from the block's end in
 * item order (item 0 last).
 */
struct acfs_item {
	struct acfs_disk_key key;
	__le16 offset;
	__le16 size;
} __attribute__((__packed__));

struct acfs_timespec {
	__le64 sec;
	__le32 nsec;
	__le32 reserved;
};

struct acfs_inode_item {
	__le64 size;
	__le64 blocks;		/* data blocks mapped */
	__le64 parent;		/* directories: the parent's ino */
	__le64 dir_seq;		/* directories: next ACFS_DIRINDEX_KEY index */
	__le64 flags;
	__le64 generation;
	__le32 mode;
	__le32 uid;
	__le32 gid;
	__le32 nlink;
	__le32 rdev;
	__le32 reserved0;
	struct acfs_timespec atime;
	struct acfs_timespec mtime;
	struct acfs_timespec ctime;
	struct acfs_timespec btime;
	__le64 reserved[4];
};

/* Directory entry, followed by name_len bytes of name. */
struct acfs_dirent {
	__le64 ino;
	__le64 index;		/* offset of its ACFS_DIRINDEX_KEY */
	__le16 name_len;
	__u8 type;		/* DT_* */
	__u8 reserved[5];
};

/* Blocks [offset - len + 1, offset] of the file are at [bnr, bnr + len). */
struct acfs_extent {
	__le64 bnr;
	__le32 len;
	__le32 flags;
};

/* Extended attribute, followed by the name and then the value. */
struct acfs_xattr {
	__le16 name_len;
	__le16 value_len;
	__u8 reserved[4];
};

struct acfs_branch_item {
	__le64 root;		/* block of the branch's tree root */
	__le64 parent;		/* branch it was made from, 0: none */
	__le64 flags;		/* ACFS_BRANCH_* of acfs.h */
	__le64 gen;		/* transaction that made it */
	struct acfs_timespec ctime;
	__le64 reserved[4];
	__u8 name_len;
	char name[ACFS_BRANCH_NAME_LEN];
};

#endif /* _UAPI_LINUX_AC_ACFS_TREE_H */

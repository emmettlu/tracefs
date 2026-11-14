/* SPDX-License-Identifier: GPL-2.0 */
#ifndef _FS_ACFS_H
#define _FS_ACFS_H

#include <linux/buffer_head.h>
#include <linux/fs.h>
#include <linux/rwsem.h>
#include <linux/sched/mm.h>
#include <linux/workqueue.h>
#include <linux/xarray.h>
#include <uapi/linux/ac/acfs.h>
#include <uapi/linux/ac/acfs_tree.h>

#define ACFS_NODE_HDR		sizeof(struct acfs_node_header)
#define ACFS_PTRS_MAX		((ACFS_BLOCK_SIZE - ACFS_NODE_HDR) / \
				 sizeof(struct acfs_key_ptr))
#define ACFS_LEAF_SPACE		(ACFS_BLOCK_SIZE - ACFS_NODE_HDR)
#define ACFS_COMMIT_SECS	5

/* A node buffer whose contents were checked since it was read. */
enum { BH_AcfsChecked = BH_PrivateStart };
BUFFER_FNS(AcfsChecked, acfs_checked)

struct acfs_key {
	u64 objectid;
	u8 type;
	u64 offset;
};

/* A tree: the root tree, or a branch's file system tree. */
struct acfs_root {
	u64 bnr;
	u64 owner;		/* branch id, 0: root tree */
	bool dirty;		/* bnr changed since the last commit */
};

struct acfs_branch {
	struct acfs_fs *fs;
	refcount_t users;	/* the table, and every inode of the branch */
	u64 id;
	u64 parent;
	u64 flags;		/* ACFS_BRANCH_* */
	u64 gen;
	struct timespec64 ctime;
	char name[ACFS_BRANCH_NAME_LEN + 1];
	struct acfs_root root;
	dev_t anon_dev;		/* st_dev of its files, 0 for main */
};

struct acfs_path {
	struct buffer_head *bh[ACFS_MAX_LEVEL];
	int slot[ACFS_MAX_LEVEL];
	int top;		/* level of the root node */
};

struct acfs_fs {
	struct super_block *sb;
	/*
	 * Every tree, the allocator and the transaction. Readers of trees
	 * take it shared, everything that changes them exclusive.
	 */
	struct rw_semaphore lock;
	u64 gen;		/* the running transaction */
	u64 nr_blocks;
	u64 rc_pages;
	u64 data_start;		/* first block after the refcount table */
	u64 next_ino;
	u64 next_branch;
	u64 free_blocks;	/* neither used nor pinned */
	u64 reserve;		/* user changes fail with ENOSPC below it */
	struct acfs_root root_tree;
	u8 uuid[16];

	unsigned long *used;	/* refcount > 0, or freed by this transaction */
	u64 alloc_hint;
	u8 *rc_cur;		/* per table page: slot of the committed copy */
	struct xarray rc_dirty;	/* table pages changed by this transaction */
	struct xarray fresh;	/* blocks allocated by this transaction */
	struct xarray nodes;	/* bnr -> buffer_head of changed nodes */
	u64 *pinned;		/* freed by this transaction */
	size_t nr_pinned;
	size_t cap_pinned;
	bool dirty;

	struct xarray branches;	/* id -> struct acfs_branch */
	struct acfs_branch *main;

	atomic_t inflight;	/* data writes of this transaction */
	wait_queue_head_t inflight_wq;
	struct delayed_work commit_work;
	int error;		/* the transaction was aborted */
};

struct acfs_inode {
	struct inode vfs;
	struct acfs_branch *br;
	u64 parent;
	u64 dir_seq;
	u64 flags;
	u64 disk_size;		/* size in the inode item */
	bool item_dirty;	/* VFS inode newer than its item */
	struct timespec64 btime;
};

static inline struct acfs_fs *ACFS_FS(struct super_block *sb)
{
	return sb->s_fs_info;
}

static inline struct acfs_inode *ACFS_I(struct inode *inode)
{
	return container_of(inode, struct acfs_inode, vfs);
}

static inline bool acfs_ro(struct inode *inode)
{
	return ACFS_I(inode)->br->flags & ACFS_BRANCH_RDONLY;
}

/* Lock helpers: no fs reclaim while holding the lock (eviction takes it). */
static inline unsigned int acfs_wlock(struct acfs_fs *fs)
{
	down_write(&fs->lock);
	return memalloc_nofs_save();
}

static inline void acfs_wunlock(struct acfs_fs *fs, unsigned int nofs)
{
	memalloc_nofs_restore(nofs);
	up_write(&fs->lock);
}

static inline unsigned int acfs_rlock(struct acfs_fs *fs)
{
	down_read(&fs->lock);
	return memalloc_nofs_save();
}

static inline void acfs_runlock(struct acfs_fs *fs, unsigned int nofs)
{
	memalloc_nofs_restore(nofs);
	up_read(&fs->lock);
}

/* tree.c */
int acfs_key_cmp(const struct acfs_key *a, const struct acfs_key *b);
void acfs_key_to_disk(struct acfs_disk_key *d, const struct acfs_key *k);
void acfs_key_from_disk(struct acfs_key *k, const struct acfs_disk_key *d);
u32 acfs_csum(const void *block);
struct buffer_head *acfs_read_node(struct acfs_fs *fs, u64 bnr);
void acfs_path_init(struct acfs_path *path);
void acfs_path_release(struct acfs_path *path);
int acfs_search(struct acfs_fs *fs, struct acfs_root *root,
		const struct acfs_key *key, struct acfs_path *path,
		int ins_len, bool cow);
int acfs_search_ge(struct acfs_fs *fs, struct acfs_root *root,
		   const struct acfs_key *key, struct acfs_path *path,
		   bool cow);
int acfs_insert(struct acfs_fs *fs, struct acfs_root *root,
		const struct acfs_key *key, u32 size, struct acfs_path *path);
int acfs_insert_data(struct acfs_fs *fs, struct acfs_root *root,
		     const struct acfs_key *key, const void *data, u32 size);
int acfs_del(struct acfs_fs *fs, struct acfs_root *root,
	     struct acfs_path *path);
int acfs_item_replace(struct acfs_fs *fs, struct acfs_root *root,
		      const struct acfs_key *key, const void *buf, int size,
		      bool exists);
int acfs_lookup(struct acfs_fs *fs, struct acfs_root *root,
		const struct acfs_key *key, void *buf, u32 size);
int acfs_update(struct acfs_fs *fs, struct acfs_root *root,
		const struct acfs_key *key, const void *buf, u32 size);
int acfs_tree_drop(struct acfs_fs *fs, u64 bnr);
int acfs_new_leaf(struct acfs_fs *fs, u64 owner, u64 *bnr);
u32 acfs_item_size(struct buffer_head *bh, int slot);
void *acfs_item_ptr(struct buffer_head *bh, int slot);
void acfs_item_key(struct buffer_head *bh, int slot, struct acfs_key *key);
u32 acfs_nritems(struct buffer_head *bh);

/* alloc.c */
int acfs_alloc_init(struct acfs_fs *fs);
void acfs_alloc_free(struct acfs_fs *fs);
int acfs_alloc_block(struct acfs_fs *fs, u64 *bnr);
int acfs_alloc_run(struct acfs_fs *fs, u64 *bnr, u32 want, u32 *got);
int acfs_rc_get(struct acfs_fs *fs, u64 bnr, u32 *count);
int acfs_rc_add(struct acfs_fs *fs, u64 bnr, u64 len, int delta);
bool acfs_fresh(struct acfs_fs *fs, u64 bnr);
void acfs_node_dirty(struct acfs_fs *fs, struct buffer_head *bh);
int acfs_commit(struct acfs_fs *fs);
int acfs_commit_gen(struct acfs_fs *fs, u64 gen);
int acfs_commit_locked(struct acfs_fs *fs);
void acfs_commit_work(struct work_struct *work);
void acfs_abort(struct acfs_fs *fs, int err);
int acfs_reserve_check(struct acfs_fs *fs);
int acfs_write_super(struct acfs_fs *fs);

/* super.c */
struct acfs_branch *acfs_branch_find(struct acfs_fs *fs, u64 id,
				     const char *name);
void acfs_branch_put(struct acfs_branch *br);
struct acfs_branch *acfs_branch_load(struct acfs_fs *fs, u64 id,
				     struct acfs_branch_item *bi);
void acfs_branch_free(struct acfs_branch *br);
void acfs_branch_to_item(struct acfs_branch *br, struct acfs_branch_item *bi);
int acfs_branch_save(struct acfs_fs *fs, struct acfs_branch *br);
int acfs_process_orphans(struct acfs_fs *fs, struct acfs_branch *br);

/* inode.c */
extern const struct inode_operations acfs_file_inode_operations;
extern const struct inode_operations acfs_special_inode_operations;
extern const struct inode_operations acfs_symlink_inode_operations;
struct inode *acfs_iget(struct super_block *sb, struct acfs_branch *br,
			u64 ino);
struct inode *acfs_ilookup(struct super_block *sb, struct acfs_branch *br,
			   u64 ino);
int acfs_drop_inode(struct inode *inode);
struct inode *acfs_new_inode(struct mnt_idmap *idmap, struct inode *dir,
			     umode_t mode, dev_t rdev, const char *symname);
void acfs_discard_new(struct inode *inode);
int acfs_write_item(struct inode *inode);
int acfs_remove_inode_items(struct acfs_fs *fs, struct acfs_root *root,
			    u64 ino);
int acfs_orphan_add(struct acfs_fs *fs, struct acfs_root *root, u64 ino);
int acfs_orphan_del(struct acfs_fs *fs, struct acfs_root *root, u64 ino);
int acfs_setattr(struct mnt_idmap *idmap, struct dentry *dentry,
		 struct iattr *attr);
int acfs_getattr(struct mnt_idmap *idmap, const struct path *path,
		 struct kstat *stat, u32 request_mask, unsigned int flags);
int acfs_permission(struct mnt_idmap *idmap, struct inode *inode, int mask);
int acfs_write_inode(struct inode *inode, struct writeback_control *wbc);
void acfs_dirty_inode(struct inode *inode, int flags);
void acfs_evict_inode(struct inode *inode);

/* dir.c */
extern const struct inode_operations acfs_dir_inode_operations;
extern const struct file_operations acfs_dir_operations;
u64 acfs_name_hash(const char *name, int len);

/* file.c */
extern const struct address_space_operations acfs_aops;
extern const struct file_operations acfs_file_operations;
int acfs_truncate(struct inode *inode, loff_t old_size);
int acfs_fsync(struct file *file, loff_t start, loff_t end, int datasync);
int acfs_punch(struct acfs_fs *fs, struct acfs_root *root, struct inode *inode,
	       u64 ino, u64 first, u64 last);

/* xattr.c */
extern const struct xattr_handler * const acfs_xattr_handlers[];
ssize_t acfs_listxattr(struct dentry *dentry, char *buf, size_t size);

/* ioctl.c, diff.c */
long acfs_ioctl(struct file *file, unsigned int cmd, unsigned long arg);
long acfs_ioc_diff(struct file *file, void __user *arg);
bool acfs_may_manage(struct file *file);

#endif /* _FS_ACFS_H */

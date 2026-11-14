// SPDX-License-Identifier: GPL-2.0
/*
 * acfs inodes. An inode is (branch, ino); the same ino in two branches is
 * the same file as of the branch point. Inode numbers come from one
 * counter for the whole file system, so files made in different branches
 * never share one.
 */
#include <linux/random.h>
#include <linux/slab.h>

#include "acfs.h"

struct acfs_iget_args {
	struct acfs_branch *br;
	u64 ino;
};

static u64 acfs_ihash(struct acfs_branch *br, u64 ino)
{
	return ino ^ (br->id * 0x9e3779b97f4a7c15ULL);
}

static int acfs_itest(struct inode *inode, void *data)
{
	struct acfs_iget_args *a = data;

	return ACFS_I(inode)->br == a->br && inode->i_ino == a->ino;
}

static int acfs_iset(struct inode *inode, void *data)
{
	struct acfs_iget_args *a = data;

	inode->i_ino = a->ino;
	ACFS_I(inode)->br = a->br;
	refcount_inc(&a->br->users);
	return 0;
}

static void set_ops(struct inode *inode)
{
	switch (inode->i_mode & S_IFMT) {
	case S_IFREG:
		inode->i_op = &acfs_file_inode_operations;
		inode->i_fop = &acfs_file_operations;
		inode->i_mapping->a_ops = &acfs_aops;
		break;
	case S_IFDIR:
		inode->i_op = &acfs_dir_inode_operations;
		inode->i_fop = &acfs_dir_operations;
		break;
	case S_IFLNK:
		inode->i_op = &acfs_symlink_inode_operations;
		break;
	default:
		inode->i_op = &acfs_special_inode_operations;
		init_special_inode(inode, inode->i_mode, inode->i_rdev);
		break;
	}
}

static void ts_from_disk(struct timespec64 *ts, const struct acfs_timespec *d)
{
	ts->tv_sec = le64_to_cpu(d->sec);
	ts->tv_nsec = le32_to_cpu(d->nsec);
}

static void ts_to_disk(struct acfs_timespec *d, struct timespec64 ts)
{
	d->sec = cpu_to_le64(ts.tv_sec);
	d->nsec = cpu_to_le32(ts.tv_nsec);
	d->reserved = 0;
}

static void inode_from_item(struct inode *inode, struct acfs_inode_item *it)
{
	struct acfs_inode *ai = ACFS_I(inode);
	struct timespec64 ts;

	inode->i_mode = le32_to_cpu(it->mode);
	i_uid_write(inode, le32_to_cpu(it->uid));
	i_gid_write(inode, le32_to_cpu(it->gid));
	set_nlink(inode, le32_to_cpu(it->nlink));
	inode->i_rdev = new_decode_dev(le32_to_cpu(it->rdev));
	i_size_write(inode, le64_to_cpu(it->size));
	inode->i_blocks = le64_to_cpu(it->blocks) << (ACFS_BLOCK_SHIFT - 9);
	inode->i_generation = le64_to_cpu(it->generation);
	ts_from_disk(&ts, &it->atime);
	inode_set_atime_to_ts(inode, ts);
	ts_from_disk(&ts, &it->mtime);
	inode_set_mtime_to_ts(inode, ts);
	ts_from_disk(&ts, &it->ctime);
	inode_set_ctime_to_ts(inode, ts);
	ts_from_disk(&ai->btime, &it->btime);
	ai->parent = le64_to_cpu(it->parent);
	ai->dir_seq = le64_to_cpu(it->dir_seq);
	ai->flags = le64_to_cpu(it->flags);
	ai->disk_size = le64_to_cpu(it->size);
}

static void item_from_inode(struct acfs_inode_item *it, struct inode *inode)
{
	struct acfs_inode *ai = ACFS_I(inode);

	memset(it, 0, sizeof(*it));
	it->size = cpu_to_le64(i_size_read(inode));
	it->blocks = cpu_to_le64(inode->i_blocks >> (ACFS_BLOCK_SHIFT - 9));
	it->parent = cpu_to_le64(ai->parent);
	it->dir_seq = cpu_to_le64(ai->dir_seq);
	it->flags = cpu_to_le64(ai->flags);
	it->generation = cpu_to_le64(inode->i_generation);
	it->mode = cpu_to_le32(inode->i_mode);
	it->uid = cpu_to_le32(i_uid_read(inode));
	it->gid = cpu_to_le32(i_gid_read(inode));
	it->nlink = cpu_to_le32(inode->i_nlink);
	it->rdev = cpu_to_le32(new_encode_dev(inode->i_rdev));
	ts_to_disk(&it->atime, inode_get_atime(inode));
	ts_to_disk(&it->mtime, inode_get_mtime(inode));
	ts_to_disk(&it->ctime, inode_get_ctime(inode));
	ts_to_disk(&it->btime, ai->btime);
}

static int read_symlink(struct acfs_fs *fs, struct inode *inode)
{
	struct acfs_key key = { inode->i_ino, ACFS_INLINE_KEY, 0 };
	char *buf = kmalloc(ACFS_ITEM_MAX + 1, GFP_NOFS);
	int len;

	if (!buf)
		return -ENOMEM;
	len = acfs_lookup(fs, &ACFS_I(inode)->br->root, &key, buf,
			  ACFS_ITEM_MAX);
	if (len < 0) {
		kfree(buf);
		return len;
	}
	buf[min(len, ACFS_ITEM_MAX)] = 0;
	inode->i_link = buf;
	return 0;
}

struct inode *acfs_ilookup(struct super_block *sb, struct acfs_branch *br,
			   u64 ino)
{
	struct acfs_iget_args args = { br, ino };

	return ilookup5(sb, acfs_ihash(br, ino), acfs_itest, &args);
}

/* Inodes of a deleted branch go as soon as nobody uses them. */
int acfs_drop_inode(struct inode *inode)
{
	struct acfs_branch *br = ACFS_I(inode)->br;

	if (br && (br->flags & ACFS_BRANCH_DELETED))
		return 1;
	return inode_generic_drop(inode);
}

struct inode *acfs_iget(struct super_block *sb, struct acfs_branch *br,
			u64 ino)
{
	struct acfs_iget_args args = { br, ino };
	struct acfs_key key = { ino, ACFS_INODE_KEY, 0 };
	struct acfs_fs *fs = ACFS_FS(sb);
	struct acfs_inode_item it;
	struct inode *inode;
	unsigned int nofs;
	int err;

	inode = iget5_locked(sb, acfs_ihash(br, ino), acfs_itest, acfs_iset,
			     &args);
	if (!inode)
		return ERR_PTR(-ENOMEM);
	if (!(inode_state_read_once(inode) & I_NEW))
		return inode;
	nofs = acfs_rlock(fs);
	err = acfs_lookup(fs, &br->root, &key, &it, sizeof(it));
	if (err >= 0 && err != sizeof(it))
		err = -EUCLEAN;
	if (err >= 0) {
		inode_from_item(inode, &it);
		err = S_ISLNK(inode->i_mode) ? read_symlink(fs, inode) : 0;
	}
	acfs_runlock(fs, nofs);
	if (err) {
		if (err == -ENOENT)
			err = -EUCLEAN;
		iget_failed(inode);
		return ERR_PTR(err);
	}
	set_ops(inode);
	unlock_new_inode(inode);
	return inode;
}

/* Write the inode's item (lock held). */
int acfs_write_item(struct inode *inode)
{
	struct acfs_inode *ai = ACFS_I(inode);
	struct acfs_key key = { inode->i_ino, ACFS_INODE_KEY, 0 };
	struct acfs_fs *fs = ACFS_FS(inode->i_sb);
	struct acfs_inode_item it;
	int err;

	WRITE_ONCE(ai->item_dirty, false);
	/* clear before copying: a change racing with the copy sets it again */
	smp_mb();
	item_from_inode(&it, inode);
	err = acfs_update(fs, &ai->br->root, &key, &it, sizeof(it));
	if (err) {
		acfs_abort(fs, err);
		return err;
	}
	ai->disk_size = le64_to_cpu(it.size);
	return 0;
}

/*
 * A new inode in dir's branch with its item (lock held). It is still
 * I_NEW: the caller instantiates it, or hands it to acfs_discard_new.
 */
struct inode *acfs_new_inode(struct mnt_idmap *idmap, struct inode *dir,
			     umode_t mode, dev_t rdev, const char *symname)
{
	struct acfs_branch *br = ACFS_I(dir)->br;
	struct acfs_fs *fs = ACFS_FS(dir->i_sb);
	struct acfs_iget_args args = { br, fs->next_ino };
	struct acfs_key key = { fs->next_ino, ACFS_INODE_KEY, 0 };
	struct acfs_inode_item it;
	struct acfs_inode *ai;
	struct inode *inode;
	int err;

	inode = iget5_locked(dir->i_sb, acfs_ihash(br, args.ino), acfs_itest,
			     acfs_iset, &args);
	if (!inode)
		return ERR_PTR(-ENOMEM);
	if (WARN_ON(!(inode_state_read_once(inode) & I_NEW))) {
		iput(inode);
		return ERR_PTR(-EUCLEAN);
	}
	fs->next_ino++;
	ai = ACFS_I(inode);
	inode_init_owner(idmap, inode, dir, mode);
	ai->btime = simple_inode_init_ts(inode);
	inode->i_generation = get_random_u32();
	inode->i_rdev = rdev;
	set_nlink(inode, S_ISDIR(mode) ? 2 : 1);
	if (S_ISDIR(mode)) {
		ai->parent = dir->i_ino;
		ai->dir_seq = 2;
	}
	if (symname) {
		inode->i_link = kstrdup(symname, GFP_NOFS);
		if (!inode->i_link) {
			err = -ENOMEM;
			goto fail;
		}
		i_size_write(inode, strlen(symname));
	}
	set_ops(inode);
	item_from_inode(&it, inode);
	err = acfs_insert_data(fs, &br->root, &key, &it, sizeof(it));
	if (!err && symname) {
		key.type = ACFS_INLINE_KEY;
		err = acfs_insert_data(fs, &br->root, &key, symname,
				       strlen(symname));
	}
	if (err) {
		acfs_abort(fs, err);
		goto fail;
	}
	return inode;
fail:
	acfs_discard_new(inode);
	return ERR_PTR(err);
}

/* Drop a new inode that never made it into a directory. */
void acfs_discard_new(struct inode *inode)
{
	kfree(inode->i_link);
	inode->i_link = NULL;
	clear_nlink(inode);
	iget_failed(inode);
}

/* Remove every item of an inode, dropping its blocks (lock held). */
int acfs_remove_inode_items(struct acfs_fs *fs, struct acfs_root *root,
			    u64 ino)
{
	struct acfs_key key = { ino, 0, 0 }, k;
	struct acfs_path path;
	int ret;

	for (;;) {
		acfs_path_init(&path);
		ret = acfs_search_ge(fs, root, &key, &path, true);
		if (ret)
			return ret < 0 ? ret : 0;
		acfs_item_key(path.bh[0], path.slot[0], &k);
		if (k.objectid != ino) {
			acfs_path_release(&path);
			return 0;
		}
		if (k.type == ACFS_EXTENT_KEY) {
			struct acfs_extent *e = acfs_item_ptr(path.bh[0],
							      path.slot[0]);

			ret = acfs_rc_add(fs, le64_to_cpu(e->bnr),
					  le32_to_cpu(e->len), -1);
			if (ret) {
				acfs_path_release(&path);
				return ret;
			}
		}
		ret = acfs_del(fs, root, &path);
		if (ret)
			return ret;
	}
}

int acfs_orphan_add(struct acfs_fs *fs, struct acfs_root *root, u64 ino)
{
	struct acfs_key key = { ACFS_ORPHAN_OBJID, ACFS_ORPHAN_KEY, ino };
	int err = acfs_insert_data(fs, root, &key, NULL, 0);

	if (err == -EEXIST)
		err = 0;
	if (err)
		acfs_abort(fs, err);
	return err;
}

int acfs_orphan_del(struct acfs_fs *fs, struct acfs_root *root, u64 ino)
{
	struct acfs_key key = { ACFS_ORPHAN_OBJID, ACFS_ORPHAN_KEY, ino };
	struct acfs_path path;
	int ret;

	acfs_path_init(&path);
	ret = acfs_search(fs, root, &key, &path, 0, true);
	if (ret < 0)
		return ret;
	if (ret) {
		acfs_path_release(&path);
		return 0;
	}
	return acfs_del(fs, root, &path);
}

void acfs_dirty_inode(struct inode *inode, int flags)
{
	WRITE_ONCE(ACFS_I(inode)->item_dirty, true);
}

int acfs_write_inode(struct inode *inode, struct writeback_control *wbc)
{
	struct acfs_fs *fs = ACFS_FS(inode->i_sb);
	unsigned int nofs;
	int err = 0;

	if (acfs_ro(inode) || is_bad_inode(inode))
		return 0;
	nofs = acfs_wlock(fs);
	err = fs->error ? -EIO : acfs_write_item(inode);
	acfs_wunlock(fs, nofs);
	return err;
}

void acfs_evict_inode(struct inode *inode)
{
	struct acfs_inode *ai = ACFS_I(inode);
	struct acfs_fs *fs = ACFS_FS(inode->i_sb);
	struct acfs_branch *br = ai->br;
	unsigned int nofs;

	truncate_inode_pages_final(&inode->i_data);
	if (br && !is_bad_inode(inode) && !sb_rdonly(inode->i_sb) &&
	    !(br->flags & ACFS_BRANCH_RDONLY)) {
		nofs = acfs_wlock(fs);
		if (!fs->error && !inode->i_nlink) {
			if (!acfs_remove_inode_items(fs, &br->root, inode->i_ino))
				acfs_orphan_del(fs, &br->root, inode->i_ino);
		} else if (!fs->error && READ_ONCE(ai->item_dirty)) {
			acfs_write_item(inode);
		}
		acfs_wunlock(fs, nofs);
	}
	clear_inode(inode);
	if (br) {
		ai->br = NULL;
		acfs_branch_put(br);
	}
}

int acfs_permission(struct mnt_idmap *idmap, struct inode *inode, int mask)
{
	if ((mask & MAY_WRITE) && acfs_ro(inode) &&
	    (S_ISREG(inode->i_mode) || S_ISDIR(inode->i_mode) ||
	     S_ISLNK(inode->i_mode)))
		return -EROFS;
	return generic_permission(idmap, inode, mask);
}

int acfs_setattr(struct mnt_idmap *idmap, struct dentry *dentry,
		 struct iattr *attr)
{
	struct inode *inode = d_inode(dentry);
	struct acfs_fs *fs = ACFS_FS(inode->i_sb);
	unsigned int nofs;
	int err;

	if (acfs_ro(inode))
		return -EROFS;
	err = setattr_prepare(idmap, dentry, attr);
	if (err)
		return err;
	err = acfs_reserve_check(fs);
	if (err)
		return err;
	if ((attr->ia_valid & ATTR_SIZE) && attr->ia_size != inode->i_size) {
		loff_t old = inode->i_size;

		if (!S_ISREG(inode->i_mode))
			return -EINVAL;
		truncate_setsize(inode, attr->ia_size);
		err = acfs_truncate(inode, old);
		if (err)
			return err;
	}
	setattr_copy(idmap, inode, attr);
	nofs = acfs_wlock(fs);
	err = fs->error ? -EIO : acfs_write_item(inode);
	acfs_wunlock(fs, nofs);
	return err;
}

int acfs_getattr(struct mnt_idmap *idmap, const struct path *path,
		 struct kstat *stat, u32 request_mask, unsigned int flags)
{
	struct inode *inode = d_inode(path->dentry);
	struct acfs_inode *ai = ACFS_I(inode);

	generic_fillattr(idmap, request_mask, inode, stat);
	if (ai->br->anon_dev)
		stat->dev = ai->br->anon_dev;
	if (request_mask & STATX_BTIME) {
		stat->btime = ai->btime;
		stat->result_mask |= STATX_BTIME;
	}
	return 0;
}

const struct inode_operations acfs_file_inode_operations = {
	.setattr	= acfs_setattr,
	.getattr	= acfs_getattr,
	.permission	= acfs_permission,
	.listxattr	= acfs_listxattr,
};

const struct inode_operations acfs_special_inode_operations = {
	.setattr	= acfs_setattr,
	.getattr	= acfs_getattr,
	.permission	= acfs_permission,
	.listxattr	= acfs_listxattr,
};

const struct inode_operations acfs_symlink_inode_operations = {
	.get_link	= simple_get_link,
	.setattr	= acfs_setattr,
	.getattr	= acfs_getattr,
	.permission	= acfs_permission,
	.listxattr	= acfs_listxattr,
};

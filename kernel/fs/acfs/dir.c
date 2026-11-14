// SPDX-License-Identifier: GPL-2.0
/*
 * acfs directories. An entry is in two items: (dir, DIRENT, hash of the
 * name) holds every entry whose name has that hash, for lookup, and
 * (dir, DIRINDEX, index) holds one, for readdir in creation order (the
 * index is the readdir position). A directory's size is the sum of its
 * names' lengths, so an empty one has size 0.
 */
#include <linux/slab.h>
#include <linux/xxhash.h>

#include "acfs.h"

#define DESZ	sizeof(struct acfs_dirent)

u64 acfs_name_hash(const char *name, int len)
{
	return xxh64(name, len, 0);
}

static int dirent_find(const u8 *buf, int size, const char *name, int len,
		       struct acfs_dirent *de)
{
	int off = 0;

	while (size - off >= (int)DESZ) {
		int nl;

		memcpy(de, buf + off, DESZ);
		nl = le16_to_cpu(de->name_len);
		if (off + DESZ + nl > size)
			return -EUCLEAN;
		if (nl == len && !memcmp(buf + off + DESZ, name, len))
			return off;
		off += DESZ + nl;
	}
	return off == size ? -ENOENT : -EUCLEAN;
}

static void dirent_key(struct acfs_key *key, struct inode *dir,
		       const char *name, int len)
{
	key->objectid = dir->i_ino;
	key->type = ACFS_DIRENT_KEY;
	key->offset = acfs_name_hash(name, len);
}

/* Find name in dir (lock held). */
static int dir_find(struct acfs_fs *fs, struct inode *dir, const char *name,
		    int len, struct acfs_dirent *de)
{
	struct acfs_key key;
	u8 *buf;
	int size, ret;

	buf = kmalloc(ACFS_ITEM_MAX, GFP_NOFS);
	if (!buf)
		return -ENOMEM;
	dirent_key(&key, dir, name, len);
	size = acfs_lookup(fs, &ACFS_I(dir)->br->root, &key, buf,
			   ACFS_ITEM_MAX);
	ret = size < 0 ? size : dirent_find(buf, size, name, len, de);
	kfree(buf);
	return ret < 0 ? ret : 0;
}

static int dir_add(struct acfs_fs *fs, struct inode *dir, const char *name,
		   int len, u64 ino, u8 type)
{
	struct acfs_root *root = &ACFS_I(dir)->br->root;
	struct acfs_dirent de = {};
	struct acfs_key key;
	int size, ret;
	bool exists;
	u8 *buf;

	buf = kmalloc(ACFS_ITEM_MAX, GFP_NOFS);
	if (!buf)
		return -ENOMEM;
	dirent_key(&key, dir, name, len);
	size = acfs_lookup(fs, root, &key, buf, ACFS_ITEM_MAX);
	exists = size >= 0;
	ret = exists ? dirent_find(buf, size, name, len, &de) : size;
	if (ret >= 0)
		ret = -EEXIST;
	if (ret != -ENOENT)
		goto out;
	if (!exists)
		size = 0;
	ret = -ENOSPC;
	if (size + DESZ + len > ACFS_ITEM_MAX)
		goto out;
	memset(&de, 0, DESZ);
	de.ino = cpu_to_le64(ino);
	de.index = cpu_to_le64(ACFS_I(dir)->dir_seq++);
	de.name_len = cpu_to_le16(len);
	de.type = type;
	memcpy(buf + size, &de, DESZ);
	memcpy(buf + size + DESZ, name, len);
	ret = acfs_item_replace(fs, root, &key, buf, size + DESZ + len, exists);
	if (ret)
		goto out;
	key.type = ACFS_DIRINDEX_KEY;
	key.offset = le64_to_cpu(de.index);
	ret = acfs_insert_data(fs, root, &key, buf + size, DESZ + len);
	if (!ret)
		i_size_write(dir, dir->i_size + len);
out:
	kfree(buf);
	return ret;
}

static int dir_remove(struct acfs_fs *fs, struct inode *dir, const char *name,
		      int len)
{
	struct acfs_root *root = &ACFS_I(dir)->br->root;
	struct acfs_dirent de;
	struct acfs_path path;
	struct acfs_key key;
	int size, off, ret;
	u8 *buf;

	buf = kmalloc(ACFS_ITEM_MAX, GFP_NOFS);
	if (!buf)
		return -ENOMEM;
	dirent_key(&key, dir, name, len);
	size = acfs_lookup(fs, root, &key, buf, ACFS_ITEM_MAX);
	off = size < 0 ? size : dirent_find(buf, size, name, len, &de);
	ret = off;
	if (off < 0)
		goto out;
	memmove(buf + off, buf + off + DESZ + len, size - off - DESZ - len);
	ret = acfs_item_replace(fs, root, &key, buf, size - DESZ - len, true);
	if (ret)
		goto out;
	key.type = ACFS_DIRINDEX_KEY;
	key.offset = le64_to_cpu(de.index);
	acfs_path_init(&path);
	ret = acfs_search(fs, root, &key, &path, 0, true);
	if (ret) {
		acfs_path_release(&path);
		ret = ret < 0 ? ret : -EUCLEAN;
		goto out;
	}
	ret = acfs_del(fs, root, &path);
	if (!ret)
		i_size_write(dir, dir->i_size - len);
out:
	kfree(buf);
	return ret;
}

/* Point name in dir at another inode, keeping its position. */
static int dir_set(struct acfs_fs *fs, struct inode *dir, const char *name,
		   int len, u64 ino, u8 type)
{
	struct acfs_root *root = &ACFS_I(dir)->br->root;
	struct acfs_dirent de;
	struct acfs_key key;
	int size, off, ret;
	u8 *buf;

	buf = kmalloc(ACFS_ITEM_MAX, GFP_NOFS);
	if (!buf)
		return -ENOMEM;
	dirent_key(&key, dir, name, len);
	size = acfs_lookup(fs, root, &key, buf, ACFS_ITEM_MAX);
	off = size < 0 ? size : dirent_find(buf, size, name, len, &de);
	ret = off;
	if (off < 0)
		goto out;
	de.ino = cpu_to_le64(ino);
	de.type = type;
	memcpy(buf + off, &de, DESZ);
	ret = acfs_update(fs, root, &key, buf, size);
	if (ret)
		goto out;
	key.type = ACFS_DIRINDEX_KEY;
	key.offset = le64_to_cpu(de.index);
	ret = acfs_update(fs, root, &key, buf + off, DESZ + len);
out:
	kfree(buf);
	return ret;
}

static struct dentry *acfs_dir_lookup(struct inode *dir, struct dentry *dentry,
				      unsigned int flags)
{
	struct acfs_fs *fs = ACFS_FS(dir->i_sb);
	struct inode *inode = NULL;
	struct acfs_dirent de;
	unsigned int nofs;
	int err;

	if (dentry->d_name.len > ACFS_NAME_LEN)
		return ERR_PTR(-ENAMETOOLONG);
	nofs = acfs_rlock(fs);
	err = dir_find(fs, dir, dentry->d_name.name, dentry->d_name.len, &de);
	acfs_runlock(fs, nofs);
	if (!err)
		inode = acfs_iget(dir->i_sb, ACFS_I(dir)->br,
				  le64_to_cpu(de.ino));
	else if (err != -ENOENT)
		inode = ERR_PTR(err);
	return d_splice_alias(inode, dentry);
}

static void touch_dir(struct inode *dir)
{
	inode_set_mtime_to_ts(dir, inode_set_ctime_current(dir));
}

static int acfs_make(struct mnt_idmap *idmap, struct inode *dir,
		     struct dentry *dentry, umode_t mode, dev_t rdev,
		     const char *symname)
{
	struct acfs_fs *fs = ACFS_FS(dir->i_sb);
	struct inode *inode;
	unsigned int nofs;
	int err;

	if (symname && strlen(symname) > ACFS_ITEM_MAX)
		return -ENAMETOOLONG;
	err = acfs_reserve_check(fs);
	if (err)
		return err;
	nofs = acfs_wlock(fs);
	if (fs->error) {
		acfs_wunlock(fs, nofs);
		return -EIO;
	}
	inode = acfs_new_inode(idmap, dir, mode, rdev, symname);
	if (IS_ERR(inode)) {
		acfs_wunlock(fs, nofs);
		return PTR_ERR(inode);
	}
	err = dir_add(fs, dir, dentry->d_name.name, dentry->d_name.len,
		      inode->i_ino, fs_umode_to_dtype(mode));
	if (!err) {
		if (S_ISDIR(mode))
			inc_nlink(dir);
		touch_dir(dir);
		err = acfs_write_item(dir);
	}
	if (err)
		acfs_abort(fs, err);
	acfs_wunlock(fs, nofs);
	if (err) {
		acfs_discard_new(inode);
		return err;
	}
	d_instantiate_new(dentry, inode);
	return 0;
}

static int acfs_create(struct mnt_idmap *idmap, struct inode *dir,
		       struct dentry *dentry, umode_t mode)
{
	return acfs_make(idmap, dir, dentry, mode, 0, NULL);
}

static int acfs_mknod(struct mnt_idmap *idmap, struct inode *dir,
		      struct dentry *dentry, umode_t mode, dev_t rdev)
{
	return acfs_make(idmap, dir, dentry, mode, rdev, NULL);
}

static int acfs_symlink(struct mnt_idmap *idmap, struct inode *dir,
			struct dentry *dentry, const char *symname)
{
	return acfs_make(idmap, dir, dentry, S_IFLNK | 0777, 0, symname);
}

static struct dentry *acfs_mkdir(struct mnt_idmap *idmap, struct inode *dir,
				 struct dentry *dentry, umode_t mode)
{
	return ERR_PTR(acfs_make(idmap, dir, dentry, S_IFDIR | mode, 0, NULL));
}

static int acfs_link(struct dentry *old_dentry, struct inode *dir,
		     struct dentry *dentry)
{
	struct inode *inode = d_inode(old_dentry);
	struct acfs_fs *fs = ACFS_FS(dir->i_sb);
	unsigned int nofs;
	int err;

	if (ACFS_I(inode)->br != ACFS_I(dir)->br)
		return -EXDEV;
	err = acfs_reserve_check(fs);
	if (err)
		return err;
	nofs = acfs_wlock(fs);
	err = fs->error ? -EIO : 0;
	if (!err)
		err = dir_add(fs, dir, dentry->d_name.name, dentry->d_name.len,
			      inode->i_ino, fs_umode_to_dtype(inode->i_mode));
	if (!err && !inode->i_nlink)
		err = acfs_orphan_del(fs, &ACFS_I(dir)->br->root, inode->i_ino);
	if (!err) {
		inc_nlink(inode);
		inode_set_ctime_current(inode);
		touch_dir(dir);
		err = acfs_write_item(inode);
	}
	if (!err)
		err = acfs_write_item(dir);
	if (err && err != -EIO)
		acfs_abort(fs, err);
	acfs_wunlock(fs, nofs);
	if (err)
		return err;
	ihold(inode);
	d_instantiate(dentry, inode);
	return 0;
}

/* Take an inode's link out of dir (lock held). */
static int drop_link(struct acfs_fs *fs, struct inode *dir, struct inode *inode)
{
	if (S_ISDIR(inode->i_mode)) {
		clear_nlink(inode);
		drop_nlink(dir);
	} else {
		drop_nlink(inode);
	}
	inode_set_ctime_current(inode);
	if (!inode->i_nlink) {
		int err = acfs_orphan_add(fs, &ACFS_I(dir)->br->root,
					  inode->i_ino);

		if (err)
			return err;
	}
	return acfs_write_item(inode);
}

static int acfs_remove(struct inode *dir, struct dentry *dentry)
{
	struct inode *inode = d_inode(dentry);
	struct acfs_fs *fs = ACFS_FS(dir->i_sb);
	unsigned int nofs;
	int err;

	if (S_ISDIR(inode->i_mode) && inode->i_size)
		return -ENOTEMPTY;
	nofs = acfs_wlock(fs);
	err = fs->error ? -EIO : 0;
	if (!err)
		err = dir_remove(fs, dir, dentry->d_name.name,
				 dentry->d_name.len);
	if (!err)
		err = drop_link(fs, dir, inode);
	if (!err) {
		touch_dir(dir);
		err = acfs_write_item(dir);
	}
	if (err && err != -EIO)
		acfs_abort(fs, err);
	acfs_wunlock(fs, nofs);
	return err;
}

static int acfs_unlink(struct inode *dir, struct dentry *dentry)
{
	return acfs_remove(dir, dentry);
}

static int acfs_rmdir(struct inode *dir, struct dentry *dentry)
{
	return acfs_remove(dir, dentry);
}

static int rename_exchange(struct acfs_fs *fs, struct inode *old_dir,
			   struct dentry *old_dentry, struct inode *new_dir,
			   struct dentry *new_dentry)
{
	struct inode *old = d_inode(old_dentry), *new = d_inode(new_dentry);
	int err;

	err = dir_set(fs, old_dir, old_dentry->d_name.name,
		      old_dentry->d_name.len, new->i_ino,
		      fs_umode_to_dtype(new->i_mode));
	if (!err)
		err = dir_set(fs, new_dir, new_dentry->d_name.name,
			      new_dentry->d_name.len, old->i_ino,
			      fs_umode_to_dtype(old->i_mode));
	if (err)
		return err;
	if (old_dir != new_dir) {
		if (S_ISDIR(old->i_mode)) {
			ACFS_I(old)->parent = new_dir->i_ino;
			drop_nlink(old_dir);
			inc_nlink(new_dir);
		}
		if (S_ISDIR(new->i_mode)) {
			ACFS_I(new)->parent = old_dir->i_ino;
			drop_nlink(new_dir);
			inc_nlink(old_dir);
		}
	}
	inode_set_ctime_current(old);
	inode_set_ctime_current(new);
	err = acfs_write_item(new);
	return err ? err : acfs_write_item(old);
}

static int rename_move(struct acfs_fs *fs, struct inode *old_dir,
		       struct dentry *old_dentry, struct inode *new_dir,
		       struct dentry *new_dentry)
{
	struct inode *old = d_inode(old_dentry), *new = d_inode(new_dentry);
	int err;

	if (new) {
		err = dir_remove(fs, new_dir, new_dentry->d_name.name,
				 new_dentry->d_name.len);
		if (!err)
			err = drop_link(fs, new_dir, new);
		if (err)
			return err;
	}
	err = dir_remove(fs, old_dir, old_dentry->d_name.name,
			 old_dentry->d_name.len);
	if (!err)
		err = dir_add(fs, new_dir, new_dentry->d_name.name,
			      new_dentry->d_name.len, old->i_ino,
			      fs_umode_to_dtype(old->i_mode));
	if (err)
		return err;
	if (S_ISDIR(old->i_mode) && old_dir != new_dir) {
		ACFS_I(old)->parent = new_dir->i_ino;
		drop_nlink(old_dir);
		inc_nlink(new_dir);
	}
	inode_set_ctime_current(old);
	return acfs_write_item(old);
}

static int acfs_rename(struct mnt_idmap *idmap, struct inode *old_dir,
		       struct dentry *old_dentry, struct inode *new_dir,
		       struct dentry *new_dentry, unsigned int flags)
{
	struct inode *new = d_inode(new_dentry);
	struct acfs_fs *fs = ACFS_FS(old_dir->i_sb);
	unsigned int nofs;
	int err;

	if (flags & ~(RENAME_NOREPLACE | RENAME_EXCHANGE))
		return -EINVAL;
	if (ACFS_I(old_dir)->br != ACFS_I(new_dir)->br)
		return -EXDEV;
	if (!(flags & RENAME_EXCHANGE) && new && S_ISDIR(new->i_mode) &&
	    new->i_size)
		return -ENOTEMPTY;
	err = acfs_reserve_check(fs);
	if (err)
		return err;
	nofs = acfs_wlock(fs);
	err = fs->error ? -EIO : 0;
	if (!err && (flags & RENAME_EXCHANGE))
		err = rename_exchange(fs, old_dir, old_dentry, new_dir,
				      new_dentry);
	else if (!err)
		err = rename_move(fs, old_dir, old_dentry, new_dir, new_dentry);
	if (!err) {
		touch_dir(old_dir);
		err = acfs_write_item(old_dir);
	}
	if (!err && new_dir != old_dir) {
		touch_dir(new_dir);
		err = acfs_write_item(new_dir);
	}
	if (err && err != -EIO)
		acfs_abort(fs, err);
	acfs_wunlock(fs, nofs);
	return err;
}

static int acfs_tmpfile(struct mnt_idmap *idmap, struct inode *dir,
			struct file *file, umode_t mode)
{
	struct acfs_fs *fs = ACFS_FS(dir->i_sb);
	struct inode *inode;
	unsigned int nofs;
	int err;

	err = acfs_reserve_check(fs);
	if (err)
		return err;
	nofs = acfs_wlock(fs);
	if (fs->error) {
		acfs_wunlock(fs, nofs);
		return -EIO;
	}
	inode = acfs_new_inode(idmap, dir, mode, 0, NULL);
	err = IS_ERR(inode) ? PTR_ERR(inode) :
	      acfs_orphan_add(fs, &ACFS_I(dir)->br->root, inode->i_ino);
	acfs_wunlock(fs, nofs);
	if (IS_ERR(inode))
		return err;
	if (err) {
		acfs_discard_new(inode);
		return err;
	}
	d_tmpfile(file, inode);
	unlock_new_inode(inode);
	return finish_open_simple(file, 0);
}

/*
 * Entries are copied out under the lock and emitted after it: emitting
 * writes to user memory, which may fault on a file of this file system.
 */
static int acfs_readdir(struct file *file, struct dir_context *ctx)
{
	struct inode *dir = file_inode(file);
	struct acfs_fs *fs = ACFS_FS(dir->i_sb);
	struct acfs_root *root = &ACFS_I(dir)->br->root;
	struct acfs_path path;
	unsigned int nofs;
	int ret = 0;
	u8 *buf;

	if (!dir_emit_dots(file, ctx))
		return 0;
	buf = kmalloc(ACFS_BLOCK_SIZE, GFP_KERNEL);
	if (!buf)
		return -ENOMEM;
	for (;;) {
		struct acfs_key key = { dir->i_ino, ACFS_DIRINDEX_KEY, ctx->pos };
		int used = 0, pos = 0;
		bool end = false;

		nofs = acfs_rlock(fs);
		acfs_path_init(&path);
		ret = acfs_search_ge(fs, root, &key, &path, false);
		if (!ret) {
			struct buffer_head *bh = path.bh[0];
			int slot, n = acfs_nritems(bh);

			for (slot = path.slot[0]; slot < n; slot++) {
				u32 size = acfs_item_size(bh, slot);

				acfs_item_key(bh, slot, &key);
				if (key.objectid != dir->i_ino ||
				    key.type != ACFS_DIRINDEX_KEY) {
					end = true;
					break;
				}
				if (size < DESZ || used + 8 + size > ACFS_BLOCK_SIZE)
					break;
				memcpy(buf + used, &size, 4);
				memcpy(buf + used + 4, acfs_item_ptr(bh, slot), size);
				used += 4 + size;
			}
			acfs_path_release(&path);
		}
		acfs_runlock(fs, nofs);
		if (ret < 0)
			break;
		if (ret > 0) {
			ret = 0;
			end = true;
		}
		while (pos < used) {
			struct acfs_dirent de;
			u32 size;
			int nl;

			memcpy(&size, buf + pos, 4);
			memcpy(&de, buf + pos + 4, DESZ);
			nl = min_t(int, le16_to_cpu(de.name_len), size - DESZ);
			if (!dir_emit(ctx, buf + pos + 4 + DESZ, nl,
				      le64_to_cpu(de.ino), de.type))
				goto out;
			ctx->pos = le64_to_cpu(de.index) + 1;
			pos += 4 + size;
		}
		if (end || !used)
			break;
	}
out:
	kfree(buf);
	return ret;
}

const struct inode_operations acfs_dir_inode_operations = {
	.lookup		= acfs_dir_lookup,
	.create		= acfs_create,
	.link		= acfs_link,
	.unlink		= acfs_unlink,
	.symlink	= acfs_symlink,
	.mkdir		= acfs_mkdir,
	.rmdir		= acfs_rmdir,
	.mknod		= acfs_mknod,
	.rename		= acfs_rename,
	.tmpfile	= acfs_tmpfile,
	.setattr	= acfs_setattr,
	.getattr	= acfs_getattr,
	.permission	= acfs_permission,
	.listxattr	= acfs_listxattr,
};

const struct file_operations acfs_dir_operations = {
	.llseek		= generic_file_llseek,
	.read		= generic_read_dir,
	.iterate_shared	= acfs_readdir,
	.fsync		= acfs_fsync,
	.unlocked_ioctl	= acfs_ioctl,
	.compat_ioctl	= compat_ptr_ioctl,
};

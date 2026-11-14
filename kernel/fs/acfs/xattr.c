// SPDX-License-Identifier: GPL-2.0
/*
 * acfs extended attributes: (ino, XATTR, hash of the full name) holds
 * every attribute whose name has that hash, each an acfs_xattr header
 * followed by the name and the value.
 */
#include <linux/slab.h>
#include <linux/xattr.h>

#include "acfs.h"

#define XSZ	sizeof(struct acfs_xattr)

static int xattr_find(const u8 *buf, int size, const char *name, int nl,
		      struct acfs_xattr *x)
{
	int off = 0;

	while (size - off >= (int)XSZ) {
		int n, v;

		memcpy(x, buf + off, XSZ);
		n = le16_to_cpu(x->name_len);
		v = le16_to_cpu(x->value_len);
		if (off + XSZ + n + v > size)
			return -EUCLEAN;
		if (n == nl && !memcmp(buf + off + XSZ, name, nl))
			return off;
		off += XSZ + n + v;
	}
	return off == size ? -ENODATA : -EUCLEAN;
}

static int acfs_xattr_get(const struct xattr_handler *handler,
			  struct dentry *unused, struct inode *inode,
			  const char *name, void *value, size_t size)
{
	struct acfs_fs *fs = ACFS_FS(inode->i_sb);
	const char *full = xattr_full_name(handler, name);
	int nl = strlen(full), isz, off, ret;
	struct acfs_key key = { inode->i_ino, ACFS_XATTR_KEY,
				acfs_name_hash(full, nl) };
	struct acfs_xattr x;
	unsigned int nofs;
	u8 *buf;

	buf = kmalloc(ACFS_ITEM_MAX, GFP_KERNEL);
	if (!buf)
		return -ENOMEM;
	nofs = acfs_rlock(fs);
	isz = acfs_lookup(fs, &ACFS_I(inode)->br->root, &key, buf,
			  ACFS_ITEM_MAX);
	acfs_runlock(fs, nofs);
	off = isz < 0 ? (isz == -ENOENT ? -ENODATA : isz) :
	      xattr_find(buf, isz, full, nl, &x);
	if (off < 0) {
		ret = off;
	} else {
		ret = le16_to_cpu(x.value_len);
		if (size && ret > size)
			ret = -ERANGE;
		else if (size)
			memcpy(value, buf + off + XSZ + nl, ret);
	}
	kfree(buf);
	return ret;
}

static int acfs_xattr_set(const struct xattr_handler *handler,
			  struct mnt_idmap *idmap, struct dentry *unused,
			  struct inode *inode, const char *name,
			  const void *value, size_t size, int flags)
{
	struct acfs_fs *fs = ACFS_FS(inode->i_sb);
	struct acfs_root *root = &ACFS_I(inode)->br->root;
	const char *full = xattr_full_name(handler, name);
	int nl = strlen(full), isz, off, err;
	struct acfs_key key = { inode->i_ino, ACFS_XATTR_KEY,
				acfs_name_hash(full, nl) };
	struct acfs_xattr x;
	unsigned int nofs;
	bool exists;
	u8 *buf;

	if (acfs_ro(inode))
		return -EROFS;
	if (XSZ + nl + size > ACFS_ITEM_MAX)
		return -E2BIG;
	err = acfs_reserve_check(fs);
	if (err)
		return err;
	buf = kmalloc(ACFS_ITEM_MAX, GFP_KERNEL);
	if (!buf)
		return -ENOMEM;
	nofs = acfs_wlock(fs);
	isz = fs->error ? -EIO : acfs_lookup(fs, root, &key, buf, ACFS_ITEM_MAX);
	exists = isz >= 0;
	err = isz;
	if (!exists && isz != -ENOENT)
		goto out;
	if (!exists)
		isz = 0;
	off = exists ? xattr_find(buf, isz, full, nl, &x) : -ENODATA;
	err = off;
	if (off < 0 && off != -ENODATA)
		goto out;
	if (off >= 0) {
		int len = XSZ + nl + le16_to_cpu(x.value_len);

		err = -EEXIST;
		if (flags & XATTR_CREATE)
			goto out;
		memmove(buf + off, buf + off + len, isz - off - len);
		isz -= len;
	} else if ((flags & XATTR_REPLACE) || !value) {
		goto out;
	}
	if (value) {
		err = -ENOSPC;
		if (isz + XSZ + nl + size > ACFS_ITEM_MAX)
			goto out;
		x = (struct acfs_xattr){
			.name_len = cpu_to_le16(nl),
			.value_len = cpu_to_le16(size),
		};
		memcpy(buf + isz, &x, XSZ);
		memcpy(buf + isz + XSZ, full, nl);
		memcpy(buf + isz + XSZ + nl, value, size);
		isz += XSZ + nl + size;
	}
	err = acfs_item_replace(fs, root, &key, buf, isz, exists);
	if (!err) {
		inode_set_ctime_current(inode);
		err = acfs_write_item(inode);
	}
	if (err)
		acfs_abort(fs, err);
out:
	acfs_wunlock(fs, nofs);
	kfree(buf);
	return err;
}

static bool may_list(const char *name)
{
	if (!strncmp(name, XATTR_TRUSTED_PREFIX, XATTR_TRUSTED_PREFIX_LEN))
		return capable(CAP_SYS_ADMIN);
	return true;
}

ssize_t acfs_listxattr(struct dentry *dentry, char *list, size_t size)
{
	struct inode *inode = d_inode(dentry);
	struct acfs_fs *fs = ACFS_FS(inode->i_sb);
	struct acfs_key key = { inode->i_ino, ACFS_XATTR_KEY, 0 };
	struct acfs_path path;
	unsigned int nofs;
	ssize_t total = 0;
	int ret, isz, off;
	u8 *buf;

	buf = kmalloc(ACFS_ITEM_MAX + 1, GFP_KERNEL);
	if (!buf)
		return -ENOMEM;
	for (;;) {
		nofs = acfs_rlock(fs);
		acfs_path_init(&path);
		ret = acfs_search_ge(fs, &ACFS_I(inode)->br->root, &key, &path,
				     false);
		if (!ret) {
			acfs_item_key(path.bh[0], path.slot[0], &key);
			isz = min_t(u32, acfs_item_size(path.bh[0], path.slot[0]),
				    ACFS_ITEM_MAX);
			memcpy(buf, acfs_item_ptr(path.bh[0], path.slot[0]), isz);
			acfs_path_release(&path);
		}
		acfs_runlock(fs, nofs);
		if (ret)
			break;
		if (key.objectid != inode->i_ino || key.type != ACFS_XATTR_KEY)
			break;
		for (off = 0; isz - off >= (int)XSZ;) {
			struct acfs_xattr x;
			int n, v;

			memcpy(&x, buf + off, XSZ);
			n = le16_to_cpu(x.name_len);
			v = le16_to_cpu(x.value_len);
			if (off + XSZ + n + v > isz)
				break;
			memmove(buf + off, buf + off + XSZ, n);
			buf[off + n] = 0;
			if (may_list(buf + off)) {
				if (size && total + n + 1 > size) {
					ret = -ERANGE;
					goto out;
				}
				if (size)
					memcpy(list + total, buf + off, n + 1);
				total += n + 1;
			}
			off += XSZ + n + v;
		}
		key.offset++;
		if (!key.offset)
			break;
	}
out:
	kfree(buf);
	return ret < 0 ? ret : total;
}

static const struct xattr_handler acfs_user_handler = {
	.prefix	= XATTR_USER_PREFIX,
	.get	= acfs_xattr_get,
	.set	= acfs_xattr_set,
};

static const struct xattr_handler acfs_trusted_handler = {
	.prefix	= XATTR_TRUSTED_PREFIX,
	.get	= acfs_xattr_get,
	.set	= acfs_xattr_set,
};

static const struct xattr_handler acfs_security_handler = {
	.prefix	= XATTR_SECURITY_PREFIX,
	.get	= acfs_xattr_get,
	.set	= acfs_xattr_set,
};

const struct xattr_handler * const acfs_xattr_handlers[] = {
	&acfs_user_handler,
	&acfs_trusted_handler,
	&acfs_security_handler,
	NULL
};

// SPDX-License-Identifier: GPL-2.0
/*
 * acfs ioctls: branches. See include/uapi/linux/ac/acfs.h.
 */
#include <linux/capability.h>
#include <linux/dcache.h>
#include <linux/file.h>
#include <linux/mount.h>
#include <linux/slab.h>
#include <linux/uaccess.h>

#include "acfs.h"

#define ACFS_LIST_MAX	1024

static void fill_info(struct acfs_branch *br, struct acfs_branch_info *info)
{
	memset(info, 0, sizeof(*info));
	info->size = sizeof(*info);
	info->id = br->id;
	info->parent = br->parent;
	info->flags = br->flags;
	info->gen = br->gen;
	info->ctime_sec = br->ctime.tv_sec;
	info->ctime_nsec = br->ctime.tv_nsec;
	strscpy(info->name, br->name, sizeof(info->name));
}

static int get_args(void *dst, size_t ksize, void __user *src)
{
	u32 size;
	int err;

	if (get_user(size, (u32 __user *)src))
		return -EFAULT;
	if (size < ksize)
		return -EINVAL;
	err = copy_struct_from_user(dst, ksize, src, size);
	return err;
}

static bool name_ok(const char *name)
{
	size_t len = strnlen(name, ACFS_BRANCH_NAME_LEN + 1);

	return len && len <= ACFS_BRANCH_NAME_LEN && !strpbrk(name, ",/\n");
}

static struct acfs_branch *name_taken(struct acfs_fs *fs, const char *name)
{
	struct acfs_branch *br;
	unsigned long i;

	xa_for_each(&fs->branches, i, br)
		if (!strcmp(br->name, name))
			return br;
	return NULL;
}

/*
 * Managing branches: CAP_SYS_ADMIN, or owning the root directory of the
 * mount the fd is on (a user's own workspace).
 */
bool acfs_may_manage(struct file *file)
{
	struct inode *root = d_inode(file->f_path.mnt->mnt_root);

	return capable(CAP_SYS_ADMIN) ||
	       inode_owner_or_capable(file_mnt_idmap(file), root);
}

static struct acfs_branch *file_branch(struct file *file)
{
	struct acfs_branch *br = ACFS_I(file_inode(file))->br;

	refcount_inc(&br->users);
	return br;
}

static long branch_create(struct file *file, void __user *arg)
{
	struct acfs_branch_create __user *uc = arg;
	struct super_block *sb = file_inode(file)->i_sb;
	struct acfs_fs *fs = ACFS_FS(sb);
	struct acfs_branch *src, *br = NULL;
	struct acfs_branch_create c;
	struct acfs_branch_info info;
	struct acfs_branch_item bi;
	unsigned int nofs;
	int err;

	if (!acfs_may_manage(file))
		return -EPERM;
	err = get_args(&c, sizeof(c), arg);
	if (err)
		return err;
	if ((c.flags & ~ACFS_BRANCH_RDONLY) || !name_ok(c.name))
		return -EINVAL;
	err = acfs_reserve_check(fs);
	if (err)
		return err;
	src = c.src ? acfs_branch_find(fs, c.src, NULL) : file_branch(file);
	if (!src)
		return -ENOENT;

	down_read(&sb->s_umount);
	err = sync_filesystem(sb);
	up_read(&sb->s_umount);
	if (err)
		goto out;

	nofs = acfs_wlock(fs);
	err = fs->error ? -EIO : 0;
	if (!err && name_taken(fs, c.name))
		err = -EEXIST;
	if (err)
		goto unlock;
	memset(&bi, 0, sizeof(bi));
	bi.root = cpu_to_le64(src->root.bnr);
	bi.parent = cpu_to_le64(src->id);
	bi.flags = cpu_to_le64(c.flags);
	bi.gen = cpu_to_le64(fs->gen);
	bi.name_len = strlen(c.name);
	memcpy(bi.name, c.name, bi.name_len);
	br = acfs_branch_load(fs, fs->next_branch, &bi);
	if (IS_ERR(br)) {
		err = PTR_ERR(br);
		br = NULL;
		goto unlock;
	}
	ktime_get_coarse_real_ts64(&br->ctime);
	err = acfs_rc_add(fs, src->root.bnr, 1, 1);
	if (!err)
		err = acfs_branch_save(fs, br);
	if (!err)
		err = xa_insert(&fs->branches, br->id, br, GFP_NOFS);
	if (err) {
		acfs_abort(fs, err);
		acfs_branch_free(br);
		goto unlock;
	}
	fs->next_branch++;
	if (!(br->flags & ACFS_BRANCH_RDONLY))
		err = acfs_process_orphans(fs, br);
	if (!err)
		err = acfs_commit_locked(fs);
	fill_info(br, &info);
unlock:
	acfs_wunlock(fs, nofs);
out:
	acfs_branch_put(src);
	if (!err && copy_to_user(&uc->info, &info, sizeof(info)))
		err = -EFAULT;
	return err;
}

/* Let go of the unused dentries, and so inodes, of a deleted branch. */
static void branch_shrink(struct super_block *sb, struct acfs_branch *br)
{
	struct inode *inode = acfs_ilookup(sb, br, ACFS_ROOT_INO);
	struct dentry *dentry;

	if (!inode)
		return;
	dentry = d_find_any_alias(inode);
	if (dentry) {
		shrink_dcache_parent(dentry);
		spin_lock(&dentry->d_lock);
		if (dentry->d_lockref.count == 1)
			__d_drop(dentry);
		spin_unlock(&dentry->d_lock);
		dput(dentry);
	}
	iput(inode);
}

static long branch_delete(struct file *file, void __user *arg)
{
	struct acfs_fs *fs = ACFS_FS(file_inode(file)->i_sb);
	struct acfs_branch_delete d;
	struct acfs_branch *br;
	unsigned int nofs;
	int err;

	if (!acfs_may_manage(file))
		return -EPERM;
	err = get_args(&d, sizeof(d), arg);
	if (err)
		return err;
	if (d.id == ACFS_MAIN_BRANCH)
		return -EBUSY;
	nofs = acfs_wlock(fs);
	br = xa_load(&fs->branches, d.id);
	if (fs->error)
		err = -EIO;
	else if (!br)
		err = -ENOENT;
	if (!err) {
		br->flags |= ACFS_BRANCH_DELETED;
		err = acfs_branch_save(fs, br);
	}
	if (!err)
		xa_erase(&fs->branches, d.id);
	acfs_wunlock(fs, nofs);
	if (err)
		return err;
	branch_shrink(file_inode(file)->i_sb, br);
	acfs_branch_put(br);
	return acfs_commit(fs);
}

static long branch_get(struct file *file, void __user *arg)
{
	struct acfs_branch_get __user *ug = arg;
	struct acfs_fs *fs = ACFS_FS(file_inode(file)->i_sb);
	struct acfs_branch_info info;
	struct acfs_branch_get g;
	struct acfs_branch *br;
	int err;

	err = get_args(&g, sizeof(g), arg);
	if (err)
		return err;
	g.name[sizeof(g.name) - 1] = 0;
	if (g.id || g.name[0])
		br = acfs_branch_find(fs, g.id, g.name);
	else
		br = file_branch(file);
	if (!br)
		return -ENOENT;
	fill_info(br, &info);
	acfs_branch_put(br);
	if (copy_to_user(&ug->info, &info, sizeof(info)))
		return -EFAULT;
	return 0;
}

static long branch_list(struct file *file, void __user *arg)
{
	struct acfs_branch_list __user *ul = arg;
	struct acfs_fs *fs = ACFS_FS(file_inode(file)->i_sb);
	struct acfs_branch_info *infos;
	struct acfs_branch_list l;
	struct acfs_branch *br;
	unsigned long i;
	u32 n = 0;
	int err;

	err = get_args(&l, sizeof(l), arg);
	if (err)
		return err;
	l.count = min_t(u32, l.count, ACFS_LIST_MAX);
	infos = kvcalloc(max_t(u32, l.count, 1), sizeof(*infos), GFP_KERNEL);
	if (!infos)
		return -ENOMEM;
	xa_lock(&fs->branches);
	xa_for_each_start(&fs->branches, i, br, l.start) {
		if (n == l.count)
			break;
		fill_info(br, &infos[n++]);
	}
	xa_unlock(&fs->branches);
	if (copy_to_user(u64_to_user_ptr(l.ptr), infos, n * sizeof(*infos)) ||
	    put_user(n, &ul->count))
		err = -EFAULT;
	kvfree(infos);
	return err;
}

/* A read-only fd of a branch's root directory, on the fd's mount. */
static long branch_open(struct file *file, void __user *arg)
{
	struct super_block *sb = file_inode(file)->i_sb;
	struct acfs_fs *fs = ACFS_FS(sb);
	struct acfs_branch_open o;
	struct acfs_branch *br;
	struct inode *inode;
	struct dentry *dentry;
	struct path path;
	struct file *f;
	int fd, err;

	if (!acfs_may_manage(file))
		return -EPERM;
	err = get_args(&o, sizeof(o), arg);
	if (err)
		return err;
	if (o.flags & ~O_CLOEXEC)
		return -EINVAL;
	o.name[sizeof(o.name) - 1] = 0;
	br = acfs_branch_find(fs, o.id, o.name[0] ? o.name : NULL);
	if (!br)
		return -ENOENT;
	inode = acfs_iget(sb, br, ACFS_ROOT_INO);
	acfs_branch_put(br);
	if (IS_ERR(inode))
		return PTR_ERR(inode);
	dentry = d_obtain_root(inode);
	if (IS_ERR(dentry))
		return PTR_ERR(dentry);
	path.mnt = file->f_path.mnt;
	path.dentry = dentry;
	fd = get_unused_fd_flags(o.flags);
	if (fd < 0) {
		dput(dentry);
		return fd;
	}
	f = dentry_open(&path, O_RDONLY | O_DIRECTORY | O_LARGEFILE, current_cred());
	dput(dentry);
	if (IS_ERR(f)) {
		put_unused_fd(fd);
		return PTR_ERR(f);
	}
	fd_install(fd, f);
	return fd;
}

long acfs_ioctl(struct file *file, unsigned int cmd, unsigned long arg)
{
	void __user *argp = (void __user *)arg;

	switch (cmd) {
	case ACFS_IOC_BRANCH_CREATE:
		return branch_create(file, argp);
	case ACFS_IOC_BRANCH_DELETE:
		return branch_delete(file, argp);
	case ACFS_IOC_BRANCH_GET:
		return branch_get(file, argp);
	case ACFS_IOC_BRANCH_LIST:
		return branch_list(file, argp);
	case ACFS_IOC_DIFF:
		return acfs_ioc_diff(file, argp);
	case ACFS_IOC_BRANCH_OPEN:
		return branch_open(file, argp);
	}
	return -ENOTTY;
}

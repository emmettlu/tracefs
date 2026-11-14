// SPDX-License-Identifier: GPL-2.0
/*
 * acfs: the Agent Computer File System.
 *
 * One file system holds branches: trees of files that share blocks until
 * they change them. A device has one super_block; mounting it with
 * branch=NAME gives that branch's root (main by default). See
 * Documentation/userspace-api/ac/acfs.rst.
 */
#include <linux/blkdev.h>
#include <linux/fs_context.h>
#include <linux/fs_parser.h>
#include <linux/module.h>
#include <linux/slab.h>
#include <linux/statfs.h>

#include "acfs.h"

static struct kmem_cache *acfs_inode_cachep;

struct acfs_mount_opts {
	char *branch;
	u64 branch_id;
};

void acfs_branch_free(struct acfs_branch *br)
{
	if (br->anon_dev)
		free_anon_bdev(br->anon_dev);
	kfree(br);
}

void acfs_branch_to_item(struct acfs_branch *br, struct acfs_branch_item *bi)
{
	memset(bi, 0, sizeof(*bi));
	bi->root = cpu_to_le64(br->root.bnr);
	bi->parent = cpu_to_le64(br->parent);
	bi->flags = cpu_to_le64(br->flags);
	bi->gen = cpu_to_le64(br->gen);
	bi->ctime.sec = cpu_to_le64(br->ctime.tv_sec);
	bi->ctime.nsec = cpu_to_le32(br->ctime.tv_nsec);
	bi->name_len = strlen(br->name);
	memcpy(bi->name, br->name, bi->name_len);
}

/* Write a branch's item into the root tree (lock held). */
int acfs_branch_save(struct acfs_fs *fs, struct acfs_branch *br)
{
	struct acfs_key key = { br->id, ACFS_BRANCH_KEY, 0 };
	struct acfs_branch_item bi;
	int err;

	acfs_branch_to_item(br, &bi);
	err = acfs_update(fs, &fs->root_tree, &key, &bi, sizeof(bi));
	if (err == -ENOENT)
		err = acfs_insert_data(fs, &fs->root_tree, &key, &bi,
				       sizeof(bi));
	if (err)
		acfs_abort(fs, err);
	return err;
}

/* Free a deleted branch's tree and item (lock held). */
static int branch_drop(struct acfs_fs *fs, struct acfs_branch *br)
{
	struct acfs_key key = { br->id, ACFS_BRANCH_KEY, 0 };
	struct acfs_path path;
	int err;

	err = acfs_tree_drop(fs, br->root.bnr);
	if (err)
		goto out;
	acfs_path_init(&path);
	err = acfs_search(fs, &fs->root_tree, &key, &path, 0, true);
	if (err > 0) {
		acfs_path_release(&path);
		err = 0;
	} else if (!err) {
		err = acfs_del(fs, &fs->root_tree, &path);
	}
out:
	if (err)
		acfs_abort(fs, err);
	return err;
}

/*
 * A branch with a reference, by id, else by name (lock held or not:
 * the table changes under the lock, entries are freed after it).
 */
struct acfs_branch *acfs_branch_find(struct acfs_fs *fs, u64 id,
				     const char *name)
{
	struct acfs_branch *br;
	unsigned long i;

	xa_lock(&fs->branches);
	if (id) {
		br = xa_load(&fs->branches, id);
	} else {
		br = NULL;
		xa_for_each(&fs->branches, i, br)
			if (!strcmp(br->name, name))
				break;
	}
	if (br && !refcount_inc_not_zero(&br->users))
		br = NULL;
	xa_unlock(&fs->branches);
	return br;
}

void acfs_branch_put(struct acfs_branch *br)
{
	struct acfs_fs *fs = br->fs;
	unsigned int nofs;

	if (!refcount_dec_and_test(&br->users))
		return;
	if ((br->flags & ACFS_BRANCH_DELETED) && !sb_rdonly(fs->sb)) {
		nofs = acfs_wlock(fs);
		if (!fs->error)
			branch_drop(fs, br);
		acfs_wunlock(fs, nofs);
	}
	acfs_branch_free(br);
}

/* Delete the inodes of a branch's orphan list (lock held). */
int acfs_process_orphans(struct acfs_fs *fs, struct acfs_branch *br)
{
	struct acfs_key key = { ACFS_ORPHAN_OBJID, ACFS_ORPHAN_KEY, 0 };
	struct acfs_path path;
	int ret;

	for (;;) {
		acfs_path_init(&path);
		ret = acfs_search_ge(fs, &br->root, &key, &path, false);
		if (ret)
			return ret < 0 ? ret : 0;
		acfs_item_key(path.bh[0], path.slot[0], &key);
		acfs_path_release(&path);
		if (key.objectid != ACFS_ORPHAN_OBJID ||
		    key.type != ACFS_ORPHAN_KEY)
			return 0;
		ret = acfs_remove_inode_items(fs, &br->root, key.offset);
		if (!ret)
			ret = acfs_orphan_del(fs, &br->root, key.offset);
		if (ret)
			return ret;
	}
}

struct acfs_branch *acfs_branch_load(struct acfs_fs *fs, u64 id,
				     struct acfs_branch_item *bi)
{
	struct acfs_branch *br = kzalloc_obj(*br);
	int err;

	if (!br)
		return ERR_PTR(-ENOMEM);
	br->fs = fs;
	refcount_set(&br->users, 1);
	br->id = id;
	br->parent = le64_to_cpu(bi->parent);
	br->flags = le64_to_cpu(bi->flags);
	br->gen = le64_to_cpu(bi->gen);
	br->ctime.tv_sec = le64_to_cpu(bi->ctime.sec);
	br->ctime.tv_nsec = le32_to_cpu(bi->ctime.nsec);
	memcpy(br->name, bi->name, min_t(u8, bi->name_len,
					 ACFS_BRANCH_NAME_LEN));
	br->root.bnr = le64_to_cpu(bi->root);
	br->root.owner = id;
	if (id != ACFS_MAIN_BRANCH) {
		err = get_anon_bdev(&br->anon_dev);
		if (err) {
			kfree(br);
			return ERR_PTR(err);
		}
	}
	return br;
}

static int load_branches(struct acfs_fs *fs)
{
	struct acfs_key key = { 0, ACFS_BRANCH_KEY, 0 };
	struct acfs_branch_item bi;
	struct acfs_branch *br;
	struct acfs_path path;
	int ret;

	for (;;) {
		acfs_path_init(&path);
		ret = acfs_search_ge(fs, &fs->root_tree, &key, &path, false);
		if (ret)
			return ret < 0 ? ret : 0;
		acfs_item_key(path.bh[0], path.slot[0], &key);
		if (key.type != ACFS_BRANCH_KEY ||
		    acfs_item_size(path.bh[0], path.slot[0]) != sizeof(bi)) {
			acfs_path_release(&path);
			return -EUCLEAN;
		}
		memcpy(&bi, acfs_item_ptr(path.bh[0], path.slot[0]), sizeof(bi));
		acfs_path_release(&path);
		if (!(le64_to_cpu(bi.flags) & ACFS_BRANCH_DELETED)) {
			br = acfs_branch_load(fs, key.objectid, &bi);
			if (IS_ERR(br))
				return PTR_ERR(br);
			ret = xa_insert(&fs->branches, br->id, br, GFP_KERNEL);
			if (ret) {
				acfs_branch_free(br);
				return ret;
			}
		}
		key.objectid++;
	}
}

static int drop_deleted(struct acfs_fs *fs)
{
	struct acfs_key key = { 0, ACFS_BRANCH_KEY, 0 };
	struct acfs_branch_item bi;
	struct acfs_branch *br;
	int ret;

	for (;;) {
		struct acfs_path path;

		acfs_path_init(&path);
		ret = acfs_search_ge(fs, &fs->root_tree, &key, &path, false);
		if (ret)
			return ret < 0 ? ret : 0;
		acfs_item_key(path.bh[0], path.slot[0], &key);
		memcpy(&bi, acfs_item_ptr(path.bh[0], path.slot[0]), sizeof(bi));
		acfs_path_release(&path);
		if (le64_to_cpu(bi.flags) & ACFS_BRANCH_DELETED) {
			br = acfs_branch_load(fs, key.objectid, &bi);
			if (IS_ERR(br))
				return PTR_ERR(br);
			ret = branch_drop(fs, br);
			acfs_branch_free(br);
			if (ret)
				return ret;
		}
		key.objectid++;
	}
}

static int read_super(struct acfs_fs *fs, struct acfs_super *out)
{
	struct super_block *sb = fs->sb;
	u64 best_gen = 0, dev_blocks;
	bool found = false;
	int s;

	for (s = 0; s < ACFS_SB_SLOTS; s++) {
		struct buffer_head *bh = sb_bread(sb, s);
		struct acfs_super *d;

		if (!bh)
			continue;
		d = (void *)bh->b_data;
		if (le32_to_cpu(d->magic) == ACFS_SUPER_MAGIC &&
		    le32_to_cpu(d->csum) == acfs_csum(d) &&
		    (!found || le64_to_cpu(d->gen) > best_gen)) {
			memcpy(out, d, sizeof(*out));
			best_gen = le64_to_cpu(d->gen);
			found = true;
		}
		brelse(bh);
	}
	if (!found)
		return -EINVAL;
	if (le32_to_cpu(out->version) != ACFS_VERSION ||
	    le32_to_cpu(out->block_size) != ACFS_BLOCK_SIZE) {
		pr_err("acfs (%pg): unsupported version %u or block size %u\n",
		       sb->s_bdev, le32_to_cpu(out->version),
		       le32_to_cpu(out->block_size));
		return -EINVAL;
	}
	dev_blocks = bdev_nr_bytes(sb->s_bdev) >> ACFS_BLOCK_SHIFT;
	fs->nr_blocks = le64_to_cpu(out->nr_blocks);
	fs->rc_pages = le64_to_cpu(out->rc_pages);
	if (fs->nr_blocks > dev_blocks ||
	    fs->rc_pages != DIV_ROUND_UP(fs->nr_blocks, ACFS_RC_PER_PAGE) ||
	    ACFS_RC_START + 2 * fs->rc_pages + 2 > fs->nr_blocks) {
		pr_err("acfs (%pg): bad geometry\n", sb->s_bdev);
		return -EUCLEAN;
	}
	fs->gen = le64_to_cpu(out->gen) + 1;
	fs->data_start = ACFS_RC_START + 2 * fs->rc_pages;
	fs->root_tree.bnr = le64_to_cpu(out->root_tree);
	fs->next_ino = le64_to_cpu(out->next_ino);
	fs->next_branch = le64_to_cpu(out->next_branch);
	memcpy(fs->uuid, out->uuid, sizeof(fs->uuid));
	return 0;
}

static void fs_free(struct acfs_fs *fs)
{
	struct acfs_branch *br;
	unsigned long i;

	xa_for_each(&fs->branches, i, br)
		acfs_branch_free(br);
	xa_destroy(&fs->branches);
	acfs_alloc_free(fs);
	kfree(fs);
}

static const struct super_operations acfs_super_ops;

static int acfs_fill_super(struct super_block *sb, struct fs_context *fc)
{
	struct acfs_super ds;
	struct acfs_branch *br;
	struct acfs_fs *fs;
	struct inode *root;
	unsigned long i;
	int err;

	fs = kzalloc_obj(*fs);
	if (!fs)
		return -ENOMEM;
	fs->sb = sb;
	sb->s_fs_info = fs;
	init_rwsem(&fs->lock);
	xa_init(&fs->rc_dirty);
	xa_init(&fs->fresh);
	xa_init(&fs->nodes);
	xa_init(&fs->branches);
	atomic_set(&fs->inflight, 0);
	init_waitqueue_head(&fs->inflight_wq);
	INIT_DELAYED_WORK(&fs->commit_work, acfs_commit_work);
	fs->root_tree.owner = 0;

	err = -EINVAL;
	if (!sb_set_blocksize(sb, ACFS_BLOCK_SIZE)) {
		errorf(fc, "acfs: %pg: blocks above 4 KiB", sb->s_bdev);
		goto fail;
	}
	err = read_super(fs, &ds);
	if (err) {
		if (err == -EINVAL && !(fc->sb_flags & SB_SILENT))
			errorf(fc, "acfs: %pg: no acfs file system", sb->s_bdev);
		goto fail;
	}
	err = acfs_alloc_init(fs);
	if (err)
		goto fail;
	err = load_branches(fs);
	if (err)
		goto fail;
	fs->main = xa_load(&fs->branches, ACFS_MAIN_BRANCH);
	if (!fs->main) {
		err = -EUCLEAN;
		errorf(fc, "acfs: %pg: no main branch", sb->s_bdev);
		goto fail;
	}

	sb->s_magic = ACFS_SUPER_MAGIC;
	sb->s_op = &acfs_super_ops;
	sb->s_xattr = acfs_xattr_handlers;
	sb->s_maxbytes = MAX_LFS_FILESIZE;
	sb->s_time_gran = 1;
	super_set_uuid(sb, fs->uuid, sizeof(fs->uuid));

	if (!sb_rdonly(sb)) {
		unsigned int nofs = acfs_wlock(fs);

		err = drop_deleted(fs);
		xa_for_each(&fs->branches, i, br)
			if (!err && !(br->flags & ACFS_BRANCH_RDONLY))
				err = acfs_process_orphans(fs, br);
		if (!err)
			err = acfs_commit_locked(fs);
		acfs_wunlock(fs, nofs);
		if (err)
			goto fail;
	}

	root = acfs_iget(sb, fs->main, ACFS_ROOT_INO);
	if (IS_ERR(root)) {
		err = PTR_ERR(root);
		goto fail;
	}
	sb->s_root = d_make_root(root);
	if (!sb->s_root) {
		err = -ENOMEM;
		goto fail;
	}
	queue_delayed_work(system_long_wq, &fs->commit_work,
			   ACFS_COMMIT_SECS * HZ);
	return 0;
fail:
	sb->s_fs_info = NULL;
	fs_free(fs);
	return err;
}

static void acfs_put_super(struct super_block *sb)
{
	struct acfs_fs *fs = ACFS_FS(sb);
	struct acfs_branch *br;
	unsigned long i;

	disable_delayed_work_sync(&fs->commit_work);
	acfs_commit(fs);
	xa_for_each(&fs->branches, i, br) {
		xa_erase(&fs->branches, i);
		acfs_branch_put(br);
	}
	acfs_commit(fs);
	sb->s_fs_info = NULL;
	fs_free(fs);
}

static int acfs_sync_fs(struct super_block *sb, int wait)
{
	struct acfs_fs *fs = ACFS_FS(sb);

	if (!wait) {
		mod_delayed_work(system_long_wq, &fs->commit_work, 0);
		return 0;
	}
	return acfs_commit(fs);
}

static int acfs_statfs(struct dentry *dentry, struct kstatfs *buf)
{
	struct acfs_fs *fs = ACFS_FS(dentry->d_sb);
	u64 bfree = READ_ONCE(fs->free_blocks) + READ_ONCE(fs->nr_pinned);

	buf->f_type = ACFS_SUPER_MAGIC;
	buf->f_bsize = ACFS_BLOCK_SIZE;
	buf->f_blocks = fs->nr_blocks - fs->data_start;
	buf->f_bfree = bfree;
	buf->f_bavail = bfree > fs->reserve ? bfree - fs->reserve : 0;
	buf->f_namelen = ACFS_NAME_LEN;
	buf->f_fsid = uuid_to_fsid(fs->uuid);
	return 0;
}

static int acfs_show_options(struct seq_file *m, struct dentry *root)
{
	struct acfs_branch *br = ACFS_I(d_inode(root))->br;

	seq_printf(m, ",branch=%s", br->name);
	return 0;
}

static struct inode *acfs_alloc_inode(struct super_block *sb)
{
	struct acfs_inode *ai = alloc_inode_sb(sb, acfs_inode_cachep,
					       GFP_KERNEL);

	if (!ai)
		return NULL;
	ai->br = NULL;
	ai->parent = 0;
	ai->dir_seq = 0;
	ai->flags = 0;
	ai->disk_size = 0;
	ai->item_dirty = false;
	ai->btime = (struct timespec64){};
	return &ai->vfs;
}

static void acfs_free_inode(struct inode *inode)
{
	if (S_ISLNK(inode->i_mode))
		kfree(inode->i_link);
	kmem_cache_free(acfs_inode_cachep, ACFS_I(inode));
}

static const struct super_operations acfs_super_ops = {
	.alloc_inode	= acfs_alloc_inode,
	.free_inode	= acfs_free_inode,
	.dirty_inode	= acfs_dirty_inode,
	.write_inode	= acfs_write_inode,
	.evict_inode	= acfs_evict_inode,
	.drop_inode	= acfs_drop_inode,
	.put_super	= acfs_put_super,
	.sync_fs	= acfs_sync_fs,
	.statfs		= acfs_statfs,
	.show_options	= acfs_show_options,
};

enum { Opt_branch, Opt_branchid };

static const struct fs_parameter_spec acfs_param_spec[] = {
	fsparam_string("branch", Opt_branch),
	fsparam_u64("branchid", Opt_branchid),
	{}
};

static int acfs_parse_param(struct fs_context *fc, struct fs_parameter *param)
{
	struct acfs_mount_opts *o = fc->fs_private;
	struct fs_parse_result result;
	int opt;

	opt = fs_parse(fc, acfs_param_spec, param, &result);
	if (opt < 0)
		return opt;
	switch (opt) {
	case Opt_branch:
		kfree(o->branch);
		o->branch = param->string;
		param->string = NULL;
		break;
	case Opt_branchid:
		o->branch_id = result.uint_64;
		break;
	}
	return 0;
}

static int acfs_get_tree(struct fs_context *fc)
{
	struct acfs_mount_opts *o = fc->fs_private;
	struct super_block *sb;
	struct acfs_branch *br;
	struct dentry *dentry;
	struct inode *inode;
	int err;

	err = get_tree_bdev(fc, acfs_fill_super);
	if (err)
		return err;
	if (!o->branch && !o->branch_id)
		return 0;
	sb = fc->root->d_sb;
	br = acfs_branch_find(ACFS_FS(sb), o->branch_id,
			      o->branch ? o->branch : "");
	if (!br) {
		err = invalf(fc, "acfs: no branch %s", o->branch ? o->branch : "with that id");
		goto fail;
	}
	inode = acfs_iget(sb, br, ACFS_ROOT_INO);
	acfs_branch_put(br);
	if (IS_ERR(inode)) {
		err = PTR_ERR(inode);
		goto fail;
	}
	dentry = d_obtain_root(inode);
	if (IS_ERR(dentry)) {
		err = PTR_ERR(dentry);
		goto fail;
	}
	dput(fc->root);
	fc->root = dentry;
	return 0;
fail:
	dput(fc->root);
	fc->root = NULL;
	deactivate_locked_super(sb);
	return err;
}

static int acfs_reconfigure(struct fs_context *fc)
{
	sync_filesystem(fc->root->d_sb);
	return 0;
}

static void acfs_free_fc(struct fs_context *fc)
{
	struct acfs_mount_opts *o = fc->fs_private;

	if (o) {
		kfree(o->branch);
		kfree(o);
	}
}

static const struct fs_context_operations acfs_context_ops = {
	.parse_param	= acfs_parse_param,
	.get_tree	= acfs_get_tree,
	.reconfigure	= acfs_reconfigure,
	.free		= acfs_free_fc,
};

static int acfs_init_fs_context(struct fs_context *fc)
{
	fc->fs_private = kzalloc_obj(struct acfs_mount_opts);
	if (!fc->fs_private)
		return -ENOMEM;
	fc->ops = &acfs_context_ops;
	return 0;
}

static struct file_system_type acfs_fs_type = {
	.owner		= THIS_MODULE,
	.name		= "acfs",
	.init_fs_context = acfs_init_fs_context,
	.parameters	= acfs_param_spec,
	.kill_sb	= kill_block_super,
	.fs_flags	= FS_REQUIRES_DEV,
};
MODULE_ALIAS_FS("acfs");

static void acfs_inode_init_once(void *p)
{
	struct acfs_inode *ai = p;

	inode_init_once(&ai->vfs);
}

static int __init acfs_init(void)
{
	int err;

	acfs_inode_cachep = kmem_cache_create("acfs_inode",
					      sizeof(struct acfs_inode), 0,
					      SLAB_RECLAIM_ACCOUNT | SLAB_ACCOUNT,
					      acfs_inode_init_once);
	if (!acfs_inode_cachep)
		return -ENOMEM;
	err = register_filesystem(&acfs_fs_type);
	if (err)
		kmem_cache_destroy(acfs_inode_cachep);
	return err;
}

static void __exit acfs_exit(void)
{
	unregister_filesystem(&acfs_fs_type);
	rcu_barrier();
	kmem_cache_destroy(acfs_inode_cachep);
}

module_init(acfs_init);
module_exit(acfs_exit);
MODULE_DESCRIPTION("acfs, the Agent Computer File System");
MODULE_LICENSE("GPL");

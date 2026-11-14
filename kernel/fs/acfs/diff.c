// SPDX-License-Identifier: GPL-2.0
/*
 * acfs diff: the items that differ between two trees. Two cursors walk
 * the trees in key order; where both stand on the same node at the same
 * key, the subtree is shared and both skip it, so only unshared nodes are
 * read. Elsewhere the cursor at the higher level, or with the smaller key,
 * goes down a level; at the leaves items are compared (after O. Rodeh's
 * and btrfs' tree comparison).
 */
#include <linux/capability.h>
#include <linux/slab.h>
#include <linux/uaccess.h>

#include "acfs.h"

#define DIFF_MAX	4096

struct cursor {
	struct acfs_fs *fs;
	struct buffer_head *bh[ACFS_MAX_LEVEL];
	int slot[ACFS_MAX_LEVEL];
	int top;
	int level;
	bool end;
};

static void cursor_release(struct cursor *c)
{
	int i;

	for (i = 0; i < ACFS_MAX_LEVEL; i++) {
		brelse(c->bh[i]);
		c->bh[i] = NULL;
	}
}

static struct acfs_key_ptr *cursor_ptr(struct cursor *c)
{
	return (struct acfs_key_ptr *)(c->bh[c->level]->b_data +
				       ACFS_NODE_HDR +
				       c->slot[c->level] *
				       sizeof(struct acfs_key_ptr));
}

static void cursor_key(struct cursor *c, struct acfs_key *key)
{
	if (c->level)
		acfs_key_from_disk(key, &cursor_ptr(c)->key);
	else
		acfs_item_key(c->bh[0], c->slot[0], key);
}

/* Next position: into the subtree under the cursor, or past it. */
static int cursor_next(struct cursor *c, bool down)
{
	struct buffer_head *bh;

	if (down && c->level) {
		bh = acfs_read_node(c->fs, le64_to_cpu(cursor_ptr(c)->bnr));
		if (IS_ERR(bh))
			return PTR_ERR(bh);
		c->level--;
		c->bh[c->level] = bh;
		c->slot[c->level] = 0;
		if (acfs_nritems(bh))
			return 0;
	}
	for (;;) {
		c->slot[c->level]++;
		if (c->slot[c->level] < acfs_nritems(c->bh[c->level]))
			return 0;
		if (c->level == c->top) {
			c->end = true;
			return 0;
		}
		brelse(c->bh[c->level]);
		c->bh[c->level] = NULL;
		c->level++;
	}
}

/* Start at the first item after key. */
static int cursor_seek(struct cursor *c, struct acfs_fs *fs,
		       struct acfs_root *root, const struct acfs_key *after)
{
	struct acfs_key key = *after;
	struct acfs_path path;
	int ret, i;

	memset(c, 0, sizeof(*c));
	c->fs = fs;
	if (++key.offset == 0 && ++key.type == 0 && ++key.objectid == 0) {
		c->end = true;
		return 0;
	}
	acfs_path_init(&path);
	ret = acfs_search(fs, root, &key, &path, 0, false);
	if (ret < 0)
		return ret;
	for (i = 0; i < ACFS_MAX_LEVEL; i++) {
		c->bh[i] = path.bh[i];
		c->slot[i] = path.slot[i];
	}
	c->top = path.top;
	c->level = 0;
	if (c->slot[0] < acfs_nritems(c->bh[0]))
		return 0;
	c->slot[0]--;
	return cursor_next(c, false);
}

static bool items_equal(struct cursor *a, struct cursor *b)
{
	u32 size = acfs_item_size(a->bh[0], a->slot[0]);

	return size == acfs_item_size(b->bh[0], b->slot[0]) &&
	       !memcmp(acfs_item_ptr(a->bh[0], a->slot[0]),
		       acfs_item_ptr(b->bh[0], b->slot[0]), size);
}

static void emit(struct acfs_diff_entry *e, const struct acfs_key *key,
		 int change)
{
	memset(e, 0, sizeof(*e));
	e->objectid = key->objectid;
	e->type = key->type;
	e->offset = key->offset;
	e->change = change;
}

static int diff_trees(struct acfs_fs *fs, struct acfs_root *ra,
		      struct acfs_root *rb, struct acfs_key *pos,
		      struct acfs_diff_entry *out, u32 *count, bool *done)
{
	struct cursor a, b;
	struct acfs_key ka, kb;
	u32 n = 0;
	int ret, cmp;

	ret = cursor_seek(&a, fs, ra, pos);
	if (!ret)
		ret = cursor_seek(&b, fs, rb, pos);
	else
		memset(&b, 0, sizeof(b));
	*done = false;
	while (!ret && n < *count) {
		struct cursor *one = NULL;

		if (a.end && b.end) {
			*done = true;
			break;
		}
		if (a.end || b.end) {
			one = a.end ? &b : &a;
			if (one->level) {
				ret = cursor_next(one, true);
				continue;
			}
			cursor_key(one, pos);
			emit(&out[n++], pos, one == &b ? ACFS_DIFF_ADDED :
						      ACFS_DIFF_REMOVED);
			ret = cursor_next(one, false);
			continue;
		}
		if (a.level != b.level) {
			ret = cursor_next(a.level > b.level ? &a : &b, true);
			continue;
		}
		cursor_key(&a, &ka);
		cursor_key(&b, &kb);
		cmp = acfs_key_cmp(&ka, &kb);
		if (a.level) {
			if (!cmp && cursor_ptr(&a)->bnr == cursor_ptr(&b)->bnr) {
				ret = cursor_next(&a, false);
				if (!ret)
					ret = cursor_next(&b, false);
			} else {
				if (cmp <= 0)
					ret = cursor_next(&a, true);
				if (!ret && cmp >= 0)
					ret = cursor_next(&b, true);
			}
			continue;
		}
		if (cmp < 0) {
			*pos = ka;
			emit(&out[n++], pos, ACFS_DIFF_REMOVED);
			ret = cursor_next(&a, false);
		} else if (cmp > 0) {
			*pos = kb;
			emit(&out[n++], pos, ACFS_DIFF_ADDED);
			ret = cursor_next(&b, false);
		} else {
			if (!items_equal(&a, &b)) {
				*pos = ka;
				emit(&out[n++], pos, ACFS_DIFF_CHANGED);
			}
			ret = cursor_next(&a, false);
			if (!ret)
				ret = cursor_next(&b, false);
		}
	}
	if (!ret && n == *count && a.end && b.end)
		*done = true;
	cursor_release(&a);
	cursor_release(&b);
	*count = n;
	return ret;
}

long acfs_ioc_diff(struct file *file, void __user *arg)
{
	struct acfs_diff __user *ud = arg;
	struct acfs_fs *fs = ACFS_FS(file_inode(file)->i_sb);
	struct acfs_branch *ba = NULL, *bb = NULL;
	struct acfs_diff_entry *out;
	struct acfs_key pos;
	struct acfs_diff d;
	unsigned int nofs;
	bool done = false;
	u32 count, size;
	int err;

	if (!acfs_may_manage(file))
		return -EPERM;
	if (get_user(size, &ud->size))
		return -EFAULT;
	if (size < sizeof(d))
		return -EINVAL;
	err = copy_struct_from_user(&d, sizeof(d), arg, size);
	if (err)
		return err;
	count = min_t(u32, d.count, DIFF_MAX);
	out = kvcalloc(max_t(u32, count, 1), sizeof(*out), GFP_KERNEL);
	if (!out)
		return -ENOMEM;
	ba = acfs_branch_find(fs, d.a, NULL);
	bb = acfs_branch_find(fs, d.b, NULL);
	err = -ENOENT;
	if (!ba || !bb)
		goto out;
	pos = (struct acfs_key){ d.pos_objectid, d.pos_type, d.pos_offset };
	nofs = acfs_rlock(fs);
	err = diff_trees(fs, &ba->root, &bb->root, &pos, out, &count, &done);
	acfs_runlock(fs, nofs);
	if (err)
		goto out;
	d.count = count;
	d.pos_objectid = pos.objectid;
	d.pos_type = pos.type;
	d.pos_offset = pos.offset;
	d.done = done;
	if (copy_to_user(u64_to_user_ptr(d.entries), out,
			 count * sizeof(*out)) ||
	    copy_to_user(ud, &d, sizeof(d)))
		err = -EFAULT;
out:
	if (ba)
		acfs_branch_put(ba);
	if (bb)
		acfs_branch_put(bb);
	kvfree(out);
	return err;
}

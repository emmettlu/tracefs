// SPDX-License-Identifier: GPL-2.0
/*
 * acfs B+ trees: copy on write, with lazy reference counts (O. Rodeh,
 * "B-trees, shadowing, and clones"). A node is changed in place only when
 * this transaction wrote it and nothing else refers to it; otherwise it is
 * copied first, and when it was shared its children gain a reference.
 * Splits happen on the way down, so a parent always has room for one more
 * pointer. Nodes are not rebalanced; empty ones are removed.
 */
#include <linux/crc32.h>

#include "acfs.h"

#define ISZ	sizeof(struct acfs_item)
#define PSZ	sizeof(struct acfs_key_ptr)
#define BS	ACFS_BLOCK_SIZE

static inline struct acfs_node_header *hdr(struct buffer_head *bh)
{
	return (struct acfs_node_header *)bh->b_data;
}

static inline struct acfs_key_ptr *kptr(struct buffer_head *bh, int i)
{
	return (struct acfs_key_ptr *)(bh->b_data + ACFS_NODE_HDR + i * PSZ);
}

static inline struct acfs_item *item(struct buffer_head *bh, int i)
{
	return (struct acfs_item *)(bh->b_data + ACFS_NODE_HDR + i * ISZ);
}

u32 acfs_nritems(struct buffer_head *bh)
{
	return le16_to_cpu(hdr(bh)->nritems);
}

static inline void set_nritems(struct buffer_head *bh, u32 n)
{
	hdr(bh)->nritems = cpu_to_le16(n);
}

static inline int level_of(struct buffer_head *bh)
{
	return hdr(bh)->level;
}

static inline u32 ioff(struct buffer_head *bh, int i)
{
	return le16_to_cpu(item(bh, i)->offset);
}

u32 acfs_item_size(struct buffer_head *bh, int slot)
{
	return le16_to_cpu(item(bh, slot)->size);
}

void *acfs_item_ptr(struct buffer_head *bh, int slot)
{
	return bh->b_data + ioff(bh, slot);
}

void acfs_item_key(struct buffer_head *bh, int slot, struct acfs_key *key)
{
	acfs_key_from_disk(key, &item(bh, slot)->key);
}

int acfs_key_cmp(const struct acfs_key *a, const struct acfs_key *b)
{
	if (a->objectid != b->objectid)
		return a->objectid < b->objectid ? -1 : 1;
	if (a->type != b->type)
		return a->type < b->type ? -1 : 1;
	if (a->offset != b->offset)
		return a->offset < b->offset ? -1 : 1;
	return 0;
}

void acfs_key_to_disk(struct acfs_disk_key *d, const struct acfs_key *k)
{
	d->objectid = cpu_to_le64(k->objectid);
	d->type = k->type;
	d->offset = cpu_to_le64(k->offset);
}

void acfs_key_from_disk(struct acfs_key *k, const struct acfs_disk_key *d)
{
	k->objectid = le64_to_cpu(d->objectid);
	k->type = d->type;
	k->offset = le64_to_cpu(d->offset);
}

static int disk_key_cmp(const struct acfs_disk_key *d, const struct acfs_key *k)
{
	struct acfs_key dk;

	acfs_key_from_disk(&dk, d);
	return acfs_key_cmp(&dk, k);
}

u32 acfs_csum(const void *block)
{
	return crc32c(~0U, block + 4, BS - 4);
}

static u32 leaf_data_end(struct buffer_head *bh)
{
	u32 n = acfs_nritems(bh);

	return n ? ioff(bh, n - 1) : BS;
}

static int leaf_free(struct buffer_head *bh)
{
	return (int)leaf_data_end(bh) -
	       (int)(ACFS_NODE_HDR + acfs_nritems(bh) * ISZ);
}

static bool node_ok(struct buffer_head *bh, u64 bnr)
{
	struct acfs_node_header *h = hdr(bh);
	u32 n = le16_to_cpu(h->nritems), i, end = BS;

	if (le32_to_cpu(h->magic) != ACFS_NODE_MAGIC ||
	    le64_to_cpu(h->bnr) != bnr || h->level >= ACFS_MAX_LEVEL ||
	    le32_to_cpu(h->csum) != acfs_csum(bh->b_data))
		return false;
	if (h->level)
		return n >= 1 && n <= ACFS_PTRS_MAX;
	if (n * ISZ > ACFS_LEAF_SPACE)
		return false;
	for (i = 0; i < n; i++) {
		u32 off = ioff(bh, i), size = acfs_item_size(bh, i);

		if (off < ACFS_NODE_HDR + n * ISZ || off + size != end)
			return false;
		end = off;
	}
	return true;
}

struct buffer_head *acfs_read_node(struct acfs_fs *fs, u64 bnr)
{
	struct buffer_head *bh;

	if (bnr < fs->data_start || bnr >= fs->nr_blocks) {
		pr_err("acfs (%pg): node pointer %llu out of range\n",
		       fs->sb->s_bdev, bnr);
		return ERR_PTR(-EUCLEAN);
	}
	bh = sb_bread(fs->sb, bnr);
	if (!bh)
		return ERR_PTR(-EIO);
	if (buffer_acfs_checked(bh))
		return bh;
	if (!node_ok(bh, bnr)) {
		pr_err("acfs (%pg): node %llu is damaged\n", fs->sb->s_bdev,
		       bnr);
		brelse(bh);
		return ERR_PTR(-EUCLEAN);
	}
	set_buffer_acfs_checked(bh);
	return bh;
}

void acfs_path_init(struct acfs_path *path)
{
	memset(path, 0, sizeof(*path));
}

void acfs_path_release(struct acfs_path *path)
{
	int i;

	for (i = 0; i < ACFS_MAX_LEVEL; i++) {
		brelse(path->bh[i]);
		path->bh[i] = NULL;
	}
}

static struct buffer_head *alloc_node(struct acfs_fs *fs, u64 owner,
				      int level, int *err)
{
	struct acfs_node_header *h;
	struct buffer_head *bh;
	u64 bnr = 0;

	*err = acfs_alloc_block(fs, &bnr);
	if (*err)
		return NULL;
	bh = sb_getblk(fs->sb, bnr);
	lock_buffer(bh);
	memset(bh->b_data, 0, BS);
	set_buffer_uptodate(bh);
	set_buffer_acfs_checked(bh);
	unlock_buffer(bh);
	h = hdr(bh);
	h->magic = cpu_to_le32(ACFS_NODE_MAGIC);
	h->bnr = cpu_to_le64(bnr);
	h->gen = cpu_to_le64(fs->gen);
	h->owner = cpu_to_le64(owner);
	h->level = level;
	acfs_node_dirty(fs, bh);
	return bh;
}

int acfs_new_leaf(struct acfs_fs *fs, u64 owner, u64 *bnr)
{
	struct buffer_head *bh;
	int err;

	bh = alloc_node(fs, owner, 0, &err);
	if (!bh)
		return err;
	*bnr = bh->b_blocknr;
	brelse(bh);
	return 0;
}

/* Every child of a node that just became one more owner's. */
static int share_children(struct acfs_fs *fs, struct buffer_head *bh)
{
	u32 n = acfs_nritems(bh), i;
	int err;

	for (i = 0; i < n; i++) {
		if (level_of(bh)) {
			err = acfs_rc_add(fs, le64_to_cpu(kptr(bh, i)->bnr), 1,
					  1);
		} else if (item(bh, i)->key.type == ACFS_EXTENT_KEY) {
			struct acfs_extent *e = acfs_item_ptr(bh, i);

			err = acfs_rc_add(fs, le64_to_cpu(e->bnr),
					  le32_to_cpu(e->len), 1);
		} else {
			err = 0;
		}
		if (err)
			return err;
	}
	return 0;
}

static void set_root(struct acfs_root *root, u64 bnr)
{
	root->bnr = bnr;
	root->dirty = true;
}

/* Make path->bh[level] writable; its parent already is. */
static int cow_node(struct acfs_fs *fs, struct acfs_root *root,
		    struct acfs_path *path, int level)
{
	struct buffer_head *bh = path->bh[level], *nbh;
	u64 bnr = bh->b_blocknr;
	u32 rc;
	int err;

	err = acfs_rc_get(fs, bnr, &rc);
	if (err)
		return err;
	if (le64_to_cpu(hdr(bh)->gen) == fs->gen && rc == 1)
		return 0;
	nbh = alloc_node(fs, root->owner, level, &err);
	if (!nbh)
		return err;
	memcpy(nbh->b_data + ACFS_NODE_HDR, bh->b_data + ACFS_NODE_HDR,
	       BS - ACFS_NODE_HDR);
	hdr(nbh)->nritems = hdr(bh)->nritems;
	if (rc > 1) {
		err = share_children(fs, bh);
		if (err)
			goto out;
	}
	err = acfs_rc_add(fs, bnr, 1, -1);
	if (err)
		goto out;
	if (level == path->top) {
		set_root(root, nbh->b_blocknr);
	} else {
		struct buffer_head *parent = path->bh[level + 1];

		kptr(parent, path->slot[level + 1])->bnr =
			cpu_to_le64(nbh->b_blocknr);
		acfs_node_dirty(fs, parent);
	}
out:
	brelse(bh);
	path->bh[level] = nbh;
	return err;
}

/* First slot with a key >= key; 0 when it is equal. */
static int leaf_slot(struct buffer_head *bh, const struct acfs_key *key,
		     int *slot)
{
	int lo = 0, hi = acfs_nritems(bh);

	while (lo < hi) {
		int mid = (lo + hi) / 2;

		if (disk_key_cmp(&item(bh, mid)->key, key) < 0)
			lo = mid + 1;
		else
			hi = mid;
	}
	*slot = lo;
	if (lo < acfs_nritems(bh) && !disk_key_cmp(&item(bh, lo)->key, key))
		return 0;
	return 1;
}

/* Last slot with a key <= key, or 0. */
static int ptr_slot(struct buffer_head *bh, const struct acfs_key *key)
{
	int lo = 0, hi = acfs_nritems(bh);

	while (lo < hi) {
		int mid = (lo + hi) / 2;

		if (disk_key_cmp(&kptr(bh, mid)->key, key) <= 0)
			lo = mid + 1;
		else
			hi = mid;
	}
	return lo ? lo - 1 : 0;
}

static int new_root(struct acfs_fs *fs, struct acfs_root *root,
		    struct acfs_path *path)
{
	struct buffer_head *old = path->bh[path->top], *nr;
	struct acfs_key_ptr *p;
	int err;

	if (path->top + 1 >= ACFS_MAX_LEVEL)
		return -EFBIG;
	nr = alloc_node(fs, root->owner, path->top + 1, &err);
	if (!nr)
		return err;
	p = kptr(nr, 0);
	if (level_of(old))
		p->key = kptr(old, 0)->key;
	else
		p->key = item(old, 0)->key;
	p->bnr = cpu_to_le64(old->b_blocknr);
	set_nritems(nr, 1);
	set_root(root, nr->b_blocknr);
	path->top++;
	path->bh[path->top] = nr;
	path->slot[path->top] = 0;
	return 0;
}

/* Insert a pointer after path->slot[level] in path->bh[level]. */
static int insert_ptr(struct acfs_fs *fs, struct acfs_path *path, int level,
		      const struct acfs_disk_key *key, u64 bnr)
{
	struct buffer_head *bh = path->bh[level];
	int slot = path->slot[level] + 1;
	u32 n = acfs_nritems(bh);

	if (WARN_ON(n >= ACFS_PTRS_MAX))
		return -EUCLEAN;
	memmove(kptr(bh, slot + 1), kptr(bh, slot), (n - slot) * PSZ);
	kptr(bh, slot)->key = *key;
	kptr(bh, slot)->bnr = cpu_to_le64(bnr);
	set_nritems(bh, n + 1);
	acfs_node_dirty(fs, bh);
	return 0;
}

static int split_internal(struct acfs_fs *fs, struct acfs_root *root,
			  struct acfs_path *path, int level)
{
	struct buffer_head *bh, *right;
	u32 n, mid;
	int err;

	if (level == path->top) {
		err = new_root(fs, root, path);
		if (err)
			return err;
	}
	bh = path->bh[level];
	n = acfs_nritems(bh);
	mid = n / 2;
	right = alloc_node(fs, root->owner, level, &err);
	if (!right)
		return err;
	memcpy(kptr(right, 0), kptr(bh, mid), (n - mid) * PSZ);
	set_nritems(right, n - mid);
	set_nritems(bh, mid);
	acfs_node_dirty(fs, bh);
	err = insert_ptr(fs, path, level + 1, &kptr(right, 0)->key,
			 right->b_blocknr);
	brelse(right);
	return err;
}

static int split_leaf(struct acfs_fs *fs, struct acfs_root *root,
		      struct acfs_path *path)
{
	struct buffer_head *bh, *right;
	u32 n, used, acc, mid, i, off;
	int err;

	if (path->top == 0) {
		err = new_root(fs, root, path);
		if (err)
			return err;
	}
	bh = path->bh[0];
	n = acfs_nritems(bh);
	if (WARN_ON(n < 2))
		return -EUCLEAN;
	used = BS - leaf_data_end(bh) + n * ISZ;
	mid = 1;
	acc = ISZ + acfs_item_size(bh, 0);
	while (mid < n - 1 && acc < used / 2) {
		acc += ISZ + acfs_item_size(bh, mid);
		mid++;
	}
	right = alloc_node(fs, root->owner, 0, &err);
	if (!right)
		return err;
	off = BS;
	for (i = mid; i < n; i++) {
		struct acfs_item *it = item(right, i - mid);
		u32 size = acfs_item_size(bh, i);

		off -= size;
		memcpy(right->b_data + off, acfs_item_ptr(bh, i), size);
		it->key = item(bh, i)->key;
		it->offset = cpu_to_le16(off);
		it->size = cpu_to_le16(size);
	}
	set_nritems(right, n - mid);
	set_nritems(bh, mid);
	acfs_node_dirty(fs, bh);
	err = insert_ptr(fs, path, 1, &item(right, 0)->key, right->b_blocknr);
	brelse(right);
	return err;
}

/*
 * Find key in root. Returns 0 when found, 1 when not (path->slot[0] is
 * where it would go), or an error. ins_len > 0 makes room for an item of
 * that many bytes (header included); cow makes the path writable, and
 * must be set with ins_len.
 */
int acfs_search(struct acfs_fs *fs, struct acfs_root *root,
		const struct acfs_key *key, struct acfs_path *path,
		int ins_len, bool cow)
{
	struct buffer_head *bh;
	int level, slot, ret, err;

again:
	acfs_path_release(path);
	bh = acfs_read_node(fs, root->bnr);
	if (IS_ERR(bh))
		return PTR_ERR(bh);
	level = level_of(bh);
	path->top = level;
	path->bh[level] = bh;
	if (cow) {
		err = cow_node(fs, root, path, level);
		if (err)
			goto fail;
	}
	for (;;) {
		struct buffer_head *child;

		bh = path->bh[level];
		if (!level) {
			ret = leaf_slot(bh, key, &slot);
			path->slot[0] = slot;
			if (ins_len > 0 && leaf_free(bh) < ins_len) {
				err = split_leaf(fs, root, path);
				if (err)
					goto fail;
				goto again;
			}
			return ret;
		}
		if (ins_len > 0 && acfs_nritems(bh) >= ACFS_PTRS_MAX) {
			err = split_internal(fs, root, path, level);
			if (err)
				goto fail;
			goto again;
		}
		slot = ptr_slot(bh, key);
		path->slot[level] = slot;
		child = acfs_read_node(fs, le64_to_cpu(kptr(bh, slot)->bnr));
		if (IS_ERR(child)) {
			err = PTR_ERR(child);
			goto fail;
		}
		if (level_of(child) != level - 1) {
			brelse(child);
			err = -EUCLEAN;
			goto fail;
		}
		path->bh[--level] = child;
		if (cow) {
			err = cow_node(fs, root, path, level);
			if (err)
				goto fail;
		}
	}
fail:
	acfs_path_release(path);
	if (cow)
		acfs_abort(fs, err);
	return err;
}

/* Position path on the first item >= key. Returns 1 when there is none. */
int acfs_search_ge(struct acfs_fs *fs, struct acfs_root *root,
		   const struct acfs_key *key, struct acfs_path *path,
		   bool cow)
{
	struct acfs_key k = *key;
	int ret, level;

	for (;;) {
		ret = acfs_search(fs, root, &k, path, 0, cow);
		if (ret < 0)
			return ret;
		if (path->slot[0] < acfs_nritems(path->bh[0]))
			return 0;
		for (level = 1; level <= path->top; level++)
			if (path->slot[level] + 1 < acfs_nritems(path->bh[level]))
				break;
		if (level > path->top) {
			acfs_path_release(path);
			return 1;
		}
		acfs_key_from_disk(&k,
				   &kptr(path->bh[level], path->slot[level] + 1)->key);
	}
}

/* Insert an item of size bytes (zeroed); path points at it after. */
int acfs_insert(struct acfs_fs *fs, struct acfs_root *root,
		const struct acfs_key *key, u32 size, struct acfs_path *path)
{
	struct buffer_head *bh;
	struct acfs_item *it;
	u32 n, data_end, x, i;
	int ret, slot;

	if (size > ACFS_ITEM_MAX)
		return -E2BIG;
	ret = acfs_search(fs, root, key, path, size + ISZ, true);
	if (ret < 0)
		return ret;
	if (!ret) {
		acfs_path_release(path);
		return -EEXIST;
	}
	bh = path->bh[0];
	slot = path->slot[0];
	n = acfs_nritems(bh);
	data_end = leaf_data_end(bh);
	x = slot ? ioff(bh, slot - 1) : BS;
	memmove(bh->b_data + data_end - size, bh->b_data + data_end,
		x - data_end);
	for (i = slot; i < n; i++)
		item(bh, i)->offset = cpu_to_le16(ioff(bh, i) - size);
	memmove(item(bh, slot + 1), item(bh, slot), (n - slot) * ISZ);
	it = item(bh, slot);
	acfs_key_to_disk(&it->key, key);
	it->offset = cpu_to_le16(x - size);
	it->size = cpu_to_le16(size);
	memset(bh->b_data + x - size, 0, size);
	set_nritems(bh, n + 1);
	acfs_node_dirty(fs, bh);
	return 0;
}

int acfs_insert_data(struct acfs_fs *fs, struct acfs_root *root,
		     const struct acfs_key *key, const void *data, u32 size)
{
	struct acfs_path path;
	int err;

	acfs_path_init(&path);
	err = acfs_insert(fs, root, key, size, &path);
	if (err)
		return err;
	memcpy(acfs_item_ptr(path.bh[0], path.slot[0]), data, size);
	acfs_path_release(&path);
	return 0;
}

/* Replace the item at key (if it exists) by buf[0, size), none if 0. */
int acfs_item_replace(struct acfs_fs *fs, struct acfs_root *root,
		      const struct acfs_key *key, const void *buf, int size,
		      bool exists)
{
	struct acfs_path path;
	int ret;

	if (exists) {
		acfs_path_init(&path);
		ret = acfs_search(fs, root, key, &path, 0, true);
		if (ret) {
			acfs_path_release(&path);
			return ret < 0 ? ret : -EUCLEAN;
		}
		ret = acfs_del(fs, root, &path);
		if (ret)
			return ret;
	}
	return size ? acfs_insert_data(fs, root, key, buf, size) : 0;
}

/* Remove the pointer at path->slot[level] and the empty child it names. */
static int del_ptr(struct acfs_fs *fs, struct acfs_root *root,
		   struct acfs_path *path, int level)
{
	struct buffer_head *bh = path->bh[level];
	int slot = path->slot[level];
	u32 n = acfs_nritems(bh);
	u64 bnr;
	int err;

	err = acfs_rc_add(fs, le64_to_cpu(kptr(bh, slot)->bnr), 1, -1);
	if (err)
		return err;
	memmove(kptr(bh, slot), kptr(bh, slot + 1), (n - slot - 1) * PSZ);
	set_nritems(bh, n - 1);
	acfs_node_dirty(fs, bh);
	if (n > 1)
		return 0;
	if (level < path->top)
		return del_ptr(fs, root, path, level + 1);
	err = acfs_new_leaf(fs, root->owner, &bnr);
	if (err)
		return err;
	err = acfs_rc_add(fs, bh->b_blocknr, 1, -1);
	set_root(root, bnr);
	return err;
}

/* Delete the item path points at (from a cow search); releases path. */
int acfs_del(struct acfs_fs *fs, struct acfs_root *root,
	     struct acfs_path *path)
{
	struct buffer_head *bh = path->bh[0];
	int slot = path->slot[0];
	u32 n = acfs_nritems(bh), size, x, data_end, i;
	int err = 0;

	size = acfs_item_size(bh, slot);
	x = ioff(bh, slot);
	data_end = leaf_data_end(bh);
	memmove(bh->b_data + data_end + size, bh->b_data + data_end,
		x - data_end);
	for (i = slot + 1; i < n; i++)
		item(bh, i)->offset = cpu_to_le16(ioff(bh, i) + size);
	memmove(item(bh, slot), item(bh, slot + 1), (n - slot - 1) * ISZ);
	set_nritems(bh, n - 1);
	acfs_node_dirty(fs, bh);
	if (n == 1 && path->top > 0)
		err = del_ptr(fs, root, path, 1);
	acfs_path_release(path);
	if (err)
		acfs_abort(fs, err);
	return err;
}

/* Copy an item's data into buf; returns its size or an error. */
int acfs_lookup(struct acfs_fs *fs, struct acfs_root *root,
		const struct acfs_key *key, void *buf, u32 size)
{
	struct acfs_path path;
	u32 isize;
	int ret;

	acfs_path_init(&path);
	ret = acfs_search(fs, root, key, &path, 0, false);
	if (ret < 0)
		return ret;
	if (ret) {
		acfs_path_release(&path);
		return -ENOENT;
	}
	isize = acfs_item_size(path.bh[0], path.slot[0]);
	memcpy(buf, acfs_item_ptr(path.bh[0], path.slot[0]), min(size, isize));
	acfs_path_release(&path);
	return isize;
}

/* Overwrite an item of the same size. */
int acfs_update(struct acfs_fs *fs, struct acfs_root *root,
		const struct acfs_key *key, const void *buf, u32 size)
{
	struct acfs_path path;
	int ret;

	acfs_path_init(&path);
	ret = acfs_search(fs, root, key, &path, 0, true);
	if (ret < 0)
		return ret;
	if (ret) {
		acfs_path_release(&path);
		return -ENOENT;
	}
	if (acfs_item_size(path.bh[0], path.slot[0]) != size) {
		acfs_path_release(&path);
		return -EUCLEAN;
	}
	memcpy(acfs_item_ptr(path.bh[0], path.slot[0]), buf, size);
	acfs_node_dirty(fs, path.bh[0]);
	acfs_path_release(&path);
	return 0;
}

/* Drop one reference to a tree, freeing what nobody else refers to. */
int acfs_tree_drop(struct acfs_fs *fs, u64 bnr)
{
	struct buffer_head *bh;
	u32 rc, n, i;
	int err;

	err = acfs_rc_get(fs, bnr, &rc);
	if (err)
		return err;
	if (rc > 1)
		return acfs_rc_add(fs, bnr, 1, -1);
	bh = acfs_read_node(fs, bnr);
	if (IS_ERR(bh))
		return PTR_ERR(bh);
	n = acfs_nritems(bh);
	for (i = 0; i < n && !err; i++) {
		if (level_of(bh)) {
			err = acfs_tree_drop(fs, le64_to_cpu(kptr(bh, i)->bnr));
		} else if (item(bh, i)->key.type == ACFS_EXTENT_KEY) {
			struct acfs_extent *e = acfs_item_ptr(bh, i);

			err = acfs_rc_add(fs, le64_to_cpu(e->bnr),
					  le32_to_cpu(e->len), -1);
		}
	}
	brelse(bh);
	if (err)
		return err;
	return acfs_rc_add(fs, bnr, 1, -1);
}

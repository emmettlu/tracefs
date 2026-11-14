// SPDX-License-Identifier: GPL-2.0
/*
 * acfs block allocation, reference counts and transactions.
 *
 * Nothing the last commit refers to is overwritten before the next commit
 * lands: tree nodes and file data are copied on write, a block freed by
 * the running transaction stays used until it commits, and each refcount
 * table page has two slots, of which the transaction writes the one not
 * holding the committed copy. So every block but the superblock may reach
 * the disk early, and a commit is: write back the block device, then the
 * superblock with a preflush and FUA.
 */
#include <linux/blkdev.h>
#include <linux/slab.h>
#include <linux/vmalloc.h>

#include "acfs.h"

#define PER	ACFS_RC_PER_PAGE

static u64 rc_block(struct acfs_fs *fs, u64 page, int slot)
{
	return ACFS_RC_START + 2 * page + slot;
}

static bool rc_page_ok(struct buffer_head *bh, u64 page, u64 committed)
{
	struct acfs_rc_page *pg = (void *)bh->b_data;

	return le32_to_cpu(pg->magic) == ACFS_RC_MAGIC &&
	       le64_to_cpu(pg->page) == page &&
	       le64_to_cpu(pg->gen) <= committed &&
	       le32_to_cpu(pg->csum) == acfs_csum(bh->b_data);
}

int acfs_alloc_init(struct acfs_fs *fs)
{
	u64 committed = fs->gen - 1, page, b;

	fs->used = kvcalloc(BITS_TO_LONGS(fs->nr_blocks), sizeof(long),
			    GFP_KERNEL);
	fs->rc_cur = kvzalloc(fs->rc_pages, GFP_KERNEL);
	if (!fs->used || !fs->rc_cur)
		return -ENOMEM;
	fs->free_blocks = 0;
	for (page = 0; page < fs->rc_pages; page++) {
		struct buffer_head *bh[2], *cur;
		struct acfs_rc_page *pg;
		int s, best = -1;
		u64 gen = 0;

		for (s = 0; s < 2; s++) {
			bh[s] = sb_bread(fs->sb, rc_block(fs, page, s));
			if (bh[s] && rc_page_ok(bh[s], page, committed)) {
				pg = (void *)bh[s]->b_data;
				if (best < 0 || le64_to_cpu(pg->gen) > gen) {
					best = s;
					gen = le64_to_cpu(pg->gen);
				}
			}
		}
		if (best < 0) {
			brelse(bh[0]);
			brelse(bh[1]);
			pr_err("acfs (%pg): refcount page %llu is damaged\n",
			       fs->sb->s_bdev, page);
			return -EUCLEAN;
		}
		fs->rc_cur[page] = best;
		cur = bh[best];
		pg = (void *)cur->b_data;
		for (b = 0; b < PER && page * PER + b < fs->nr_blocks; b++) {
			if (pg->count[b])
				__set_bit(page * PER + b, fs->used);
			else
				fs->free_blocks++;
		}
		brelse(bh[0]);
		brelse(bh[1]);
	}
	fs->reserve = clamp_t(u64, fs->nr_blocks / 64, 64, 262144);
	fs->alloc_hint = fs->data_start;
	return 0;
}

void acfs_alloc_free(struct acfs_fs *fs)
{
	struct buffer_head *bh;
	unsigned long i;

	xa_for_each(&fs->nodes, i, bh)
		brelse(bh);
	xa_destroy(&fs->nodes);
	xa_for_each(&fs->rc_dirty, i, bh)
		brelse(bh);
	xa_destroy(&fs->rc_dirty);
	xa_destroy(&fs->fresh);
	kvfree(fs->used);
	kvfree(fs->rc_cur);
	kvfree(fs->pinned);
}

/* The page's working copy; write: the copy this transaction writes. */
static struct buffer_head *rc_page(struct acfs_fs *fs, u64 page, bool write)
{
	struct buffer_head *src, *dst;

	dst = xa_load(&fs->rc_dirty, page);
	if (dst) {
		get_bh(dst);
		return dst;
	}
	src = sb_bread(fs->sb, rc_block(fs, page, fs->rc_cur[page]));
	if (!src || !write)
		return src;
	dst = sb_getblk(fs->sb, rc_block(fs, page, !fs->rc_cur[page]));
	lock_buffer(dst);
	memcpy(dst->b_data, src->b_data, ACFS_BLOCK_SIZE);
	set_buffer_uptodate(dst);
	unlock_buffer(dst);
	mark_buffer_dirty(dst);
	brelse(src);
	get_bh(dst);
	xa_store(&fs->rc_dirty, page, dst, GFP_NOFS | __GFP_NOFAIL);
	return dst;
}

static int check_range(struct acfs_fs *fs, u64 bnr, u64 len)
{
	if (bnr < fs->data_start || bnr >= fs->nr_blocks ||
	    len > fs->nr_blocks - bnr) {
		pr_err("acfs (%pg): block range %llu+%llu out of range\n",
		       fs->sb->s_bdev, bnr, len);
		return -EUCLEAN;
	}
	return 0;
}

int acfs_rc_get(struct acfs_fs *fs, u64 bnr, u32 *count)
{
	struct buffer_head *bh;
	struct acfs_rc_page *pg;
	int err;

	err = check_range(fs, bnr, 1);
	if (err)
		return err;
	bh = rc_page(fs, bnr / PER, false);
	if (!bh)
		return -EIO;
	pg = (void *)bh->b_data;
	*count = le32_to_cpu(pg->count[bnr % PER]);
	brelse(bh);
	return 0;
}

bool acfs_fresh(struct acfs_fs *fs, u64 bnr)
{
	return xa_load(&fs->fresh, bnr);
}

static void pin(struct acfs_fs *fs, u64 bnr)
{
	if (fs->nr_pinned == fs->cap_pinned) {
		size_t cap = max_t(size_t, 256, fs->cap_pinned * 2);
		u64 *p = kvrealloc(fs->pinned, cap * sizeof(u64),
				   GFP_NOFS | __GFP_NOFAIL);

		fs->pinned = p;
		fs->cap_pinned = cap;
	}
	fs->pinned[fs->nr_pinned++] = bnr;
}

/*
 * A block's count dropped to zero. A node this transaction wrote is free
 * at once (after any write of it in flight); anything else waits for the
 * commit, since the committed trees or in-flight data may still use it.
 */
static void free_block(struct acfs_fs *fs, u64 bnr)
{
	struct buffer_head *bh = xa_erase(&fs->nodes, bnr);

	xa_erase(&fs->fresh, bnr);
	if (bh) {
		lock_buffer(bh);
		clear_buffer_dirty(bh);
		unlock_buffer(bh);
		brelse(bh);
		__clear_bit(bnr, fs->used);
		fs->free_blocks++;
		return;
	}
	pin(fs, bnr);
}

int acfs_rc_add(struct acfs_fs *fs, u64 bnr, u64 len, int delta)
{
	int err;

	err = check_range(fs, bnr, len);
	if (err) {
		acfs_abort(fs, err);
		return err;
	}
	while (len) {
		u64 page = bnr / PER, idx = bnr % PER, n = min(len, PER - idx);
		struct acfs_rc_page *pg;
		struct buffer_head *bh;
		u64 i;

		bh = rc_page(fs, page, true);
		if (!bh) {
			acfs_abort(fs, -EIO);
			return -EIO;
		}
		pg = (void *)bh->b_data;
		for (i = 0; i < n; i++) {
			u32 c = le32_to_cpu(pg->count[idx + i]);

			if ((delta < 0 && c < -delta) || (delta > 0 && !c) ||
			    !test_bit(bnr + i, fs->used)) {
				pr_err("acfs (%pg): refcount of block %llu is wrong\n",
				       fs->sb->s_bdev, bnr + i);
				brelse(bh);
				acfs_abort(fs, -EUCLEAN);
				return -EUCLEAN;
			}
			c += delta;
			pg->count[idx + i] = cpu_to_le32(c);
			if (!c)
				free_block(fs, bnr + i);
		}
		mark_buffer_dirty(bh);
		brelse(bh);
		bnr += n;
		len -= n;
	}
	fs->dirty = true;
	return 0;
}

/*
 * Allocate up to want consecutive blocks with count 1 (at least one):
 * the first free run at or after *bnr (a hint, 0 for none), else from
 * the start of the data area. Sets *bnr and *got.
 */
int acfs_alloc_run(struct acfs_fs *fs, u64 *bnr, u32 want, u32 *got)
{
	u64 hint = *bnr ? *bnr : fs->alloc_hint, b, end, cur;

	if (hint < fs->data_start || hint >= fs->nr_blocks)
		hint = fs->data_start;
	b = find_next_zero_bit(fs->used, fs->nr_blocks, hint);
	if (b >= fs->nr_blocks)
		b = find_next_zero_bit(fs->used, fs->nr_blocks, fs->data_start);
	if (b >= fs->nr_blocks)
		return -ENOSPC;
	end = find_next_bit(fs->used, min_t(u64, fs->nr_blocks, b + want), b);
	for (cur = b; cur < end;) {
		u64 idx = cur % PER, n = min(end - cur, PER - idx), i;
		struct buffer_head *bh = rc_page(fs, cur / PER, true);
		struct acfs_rc_page *pg;

		if (!bh) {
			acfs_abort(fs, -EIO);
			return -EIO;
		}
		pg = (void *)bh->b_data;
		for (i = 0; i < n; i++) {
			if (pg->count[idx + i]) {
				brelse(bh);
				pr_err("acfs (%pg): free block %llu has references\n",
				       fs->sb->s_bdev, cur + i);
				acfs_abort(fs, -EUCLEAN);
				return -EUCLEAN;
			}
			pg->count[idx + i] = cpu_to_le32(1);
			__set_bit(cur + i, fs->used);
			xa_store(&fs->fresh, cur + i, xa_mk_value(1),
				 GFP_NOFS | __GFP_NOFAIL);
		}
		mark_buffer_dirty(bh);
		brelse(bh);
		cur += n;
	}
	fs->free_blocks -= end - b;
	fs->alloc_hint = end;
	fs->dirty = true;
	*bnr = b;
	*got = end - b;
	return 0;
}

/* Allocate a block with count 1; *bnr is a hint, 0 for none. */
int acfs_alloc_block(struct acfs_fs *fs, u64 *bnr)
{
	u32 got;

	return acfs_alloc_run(fs, bnr, 1, &got);
}

void acfs_node_dirty(struct acfs_fs *fs, struct buffer_head *bh)
{
	if (!xa_load(&fs->nodes, bh->b_blocknr)) {
		get_bh(bh);
		xa_store(&fs->nodes, bh->b_blocknr, bh,
			 GFP_NOFS | __GFP_NOFAIL);
	}
	mark_buffer_dirty(bh);
	fs->dirty = true;
}

void acfs_abort(struct acfs_fs *fs, int err)
{
	if (fs->error)
		return;
	fs->error = err;
	pr_err("acfs (%pg): transaction %llu aborted (%d), changes since the last commit are lost\n",
	       fs->sb->s_bdev, fs->gen, err);
}

/*
 * Before a change that needs new blocks: fail with ENOSPC below the
 * reserve (what is left is for the changes in flight, deletions and
 * commits), after letting a commit return the blocks freed since.
 */
int acfs_reserve_check(struct acfs_fs *fs)
{
	if (READ_ONCE(fs->error))
		return -EIO;
	if (READ_ONCE(fs->free_blocks) > fs->reserve)
		return 0;
	if (READ_ONCE(fs->nr_pinned))
		acfs_commit(fs);
	return READ_ONCE(fs->free_blocks) > fs->reserve ? 0 : -ENOSPC;
}

int acfs_write_super(struct acfs_fs *fs)
{
	struct buffer_head *bh = sb_getblk(fs->sb, fs->gen % ACFS_SB_SLOTS);
	struct acfs_super *s = (void *)bh->b_data;
	int err;

	lock_buffer(bh);
	memset(s, 0, ACFS_BLOCK_SIZE);
	s->magic = cpu_to_le32(ACFS_SUPER_MAGIC);
	s->version = cpu_to_le32(ACFS_VERSION);
	s->block_size = cpu_to_le32(ACFS_BLOCK_SIZE);
	s->gen = cpu_to_le64(fs->gen);
	s->nr_blocks = cpu_to_le64(fs->nr_blocks);
	s->rc_pages = cpu_to_le64(fs->rc_pages);
	s->root_tree = cpu_to_le64(fs->root_tree.bnr);
	s->next_ino = cpu_to_le64(fs->next_ino);
	s->next_branch = cpu_to_le64(fs->next_branch);
	s->free_blocks = cpu_to_le64(fs->free_blocks + fs->nr_pinned);
	memcpy(s->uuid, fs->uuid, sizeof(s->uuid));
	s->csum = cpu_to_le32(acfs_csum(s));
	set_buffer_uptodate(bh);
	unlock_buffer(bh);
	mark_buffer_dirty(bh);
	err = __sync_dirty_buffer(bh, REQ_SYNC | REQ_PREFLUSH | REQ_FUA);
	brelse(bh);
	return err;
}

int acfs_commit_locked(struct acfs_fs *fs)
{
	struct buffer_head *bh;
	struct acfs_branch *br;
	unsigned long i;
	size_t p;
	int err;

	if (fs->error)
		return -EIO;
	if (sb_rdonly(fs->sb))
		return 0;
	wait_event(fs->inflight_wq, !atomic_read(&fs->inflight));
	if (!fs->dirty)
		return 0;
	xa_for_each(&fs->branches, i, br) {
		if (br->root.dirty) {
			err = acfs_branch_save(fs, br);
			if (err)
				goto abort;
		}
	}
	xa_for_each(&fs->nodes, i, bh) {
		struct acfs_node_header *h = (void *)bh->b_data;

		lock_buffer(bh);
		h->csum = cpu_to_le32(acfs_csum(bh->b_data));
		unlock_buffer(bh);
		mark_buffer_dirty(bh);
	}
	xa_for_each(&fs->rc_dirty, i, bh) {
		struct acfs_rc_page *pg = (void *)bh->b_data;

		lock_buffer(bh);
		pg->magic = cpu_to_le32(ACFS_RC_MAGIC);
		pg->gen = cpu_to_le64(fs->gen);
		pg->page = cpu_to_le64(i);
		pg->csum = cpu_to_le32(acfs_csum(bh->b_data));
		unlock_buffer(bh);
		mark_buffer_dirty(bh);
	}
	err = sync_blockdev(fs->sb->s_bdev);
	if (err)
		goto abort;
	err = acfs_write_super(fs);
	if (err)
		goto abort;

	xa_for_each(&fs->nodes, i, bh) {
		xa_erase(&fs->nodes, i);
		brelse(bh);
	}
	xa_for_each(&fs->rc_dirty, i, bh) {
		xa_erase(&fs->rc_dirty, i);
		fs->rc_cur[i] = !fs->rc_cur[i];
		brelse(bh);
	}
	for (p = 0; p < fs->nr_pinned; p++)
		__clear_bit(fs->pinned[p], fs->used);
	fs->free_blocks += fs->nr_pinned;
	fs->nr_pinned = 0;
	xa_destroy(&fs->fresh);
	xa_for_each(&fs->branches, i, br)
		br->root.dirty = false;
	fs->root_tree.dirty = false;
	fs->dirty = false;
	fs->gen++;
	return 0;
abort:
	acfs_abort(fs, err);
	return err;
}

int acfs_commit(struct acfs_fs *fs)
{
	unsigned int nofs = acfs_wlock(fs);
	int err = acfs_commit_locked(fs);

	acfs_wunlock(fs, nofs);
	return err;
}

/*
 * Commit transaction gen, unless a commit since did: callers that wait
 * for the lock together (fsync) then share one commit.
 */
int acfs_commit_gen(struct acfs_fs *fs, u64 gen)
{
	unsigned int nofs = acfs_wlock(fs);
	int err;

	if (fs->gen > gen)
		err = fs->error ? -EIO : 0;
	else
		err = acfs_commit_locked(fs);
	acfs_wunlock(fs, nofs);
	return err;
}

void acfs_commit_work(struct work_struct *work)
{
	struct acfs_fs *fs = container_of(to_delayed_work(work), struct acfs_fs,
					  commit_work);

	acfs_commit(fs);
	queue_delayed_work(system_long_wq, &fs->commit_work,
			   ACFS_COMMIT_SECS * HZ);
}

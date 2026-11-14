// SPDX-License-Identifier: GPL-2.0
/*
 * acfs file data. Writes go to the page cache; writeback puts each block
 * somewhere new, except a block that this transaction wrote and nothing
 * shares, which it overwrites. So a commit never refers to half-written
 * data, and branches keep theirs. An extent item maps file blocks
 * [offset - len + 1, offset] (it is keyed by its last block) to disk
 * blocks [bnr, bnr + len). A folio is one block.
 */
#include <linux/bio.h>
#include <linux/blkdev.h>
#include <linux/pagemap.h>
#include <linux/slab.h>
#include <linux/writeback.h>

#include "acfs.h"

#define BS	ACFS_BLOCK_SIZE

static void extent_at(struct acfs_path *path, struct acfs_key *key,
		      struct acfs_extent *e)
{
	acfs_item_key(path->bh[0], path->slot[0], key);
	memcpy(e, acfs_item_ptr(path->bh[0], path->slot[0]), sizeof(*e));
}

/* Disk block of file block lblk, 0 for a hole (lock held). */
static int map_block(struct acfs_fs *fs, struct acfs_root *root, u64 ino,
		     u64 lblk, u64 *pbn)
{
	struct acfs_key key = { ino, ACFS_EXTENT_KEY, lblk };
	struct acfs_path path;
	struct acfs_extent e;
	int ret;

	*pbn = 0;
	acfs_path_init(&path);
	ret = acfs_search_ge(fs, root, &key, &path, false);
	if (ret)
		return ret < 0 ? ret : 0;
	extent_at(&path, &key, &e);
	acfs_path_release(&path);
	if (key.objectid == ino && key.type == ACFS_EXTENT_KEY &&
	    key.offset - le32_to_cpu(e.len) + 1 <= lblk)
		*pbn = le64_to_cpu(e.bnr) + lblk -
		       (key.offset - le32_to_cpu(e.len) + 1);
	return 0;
}

static int map_folio(struct inode *inode, pgoff_t index, u64 *pbn)
{
	struct acfs_fs *fs = ACFS_FS(inode->i_sb);
	unsigned int nofs = acfs_rlock(fs);
	int err;

	err = map_block(fs, &ACFS_I(inode)->br->root, inode->i_ino, index, pbn);
	acfs_runlock(fs, nofs);
	return err;
}

static struct bio *folio_bio(struct inode *inode, struct folio *folio,
			     u64 pbn, blk_opf_t opf)
{
	struct bio *bio = bio_alloc(inode->i_sb->s_bdev, 1, opf, GFP_NOFS);

	bio->bi_iter.bi_sector = pbn << (ACFS_BLOCK_SHIFT - SECTOR_SHIFT);
	bio_add_folio_nofail(bio, folio, folio_size(folio), 0);
	return bio;
}

static void read_end_io(struct bio *bio)
{
	folio_end_read(bio_first_folio_all(bio), !bio->bi_status);
	bio_put(bio);
}

static int acfs_read_folio(struct file *file, struct folio *folio)
{
	struct inode *inode = folio->mapping->host;
	struct bio *bio;
	u64 pbn = 0;
	int err = 0;

	if (folio_pos(folio) < i_size_read(inode))
		err = map_folio(inode, folio->index, &pbn);
	if (err) {
		folio_unlock(folio);
		return err;
	}
	if (!pbn) {
		folio_zero_range(folio, 0, folio_size(folio));
		folio_end_read(folio, true);
		return 0;
	}
	bio = folio_bio(inode, folio, pbn, REQ_OP_READ);
	bio->bi_end_io = read_end_io;
	submit_bio(bio);
	return 0;
}

/* Read a locked folio in place, for a partial write. */
static int fill_folio(struct inode *inode, struct folio *folio)
{
	struct bio *bio;
	u64 pbn = 0;
	int err = 0;

	if (folio_pos(folio) < i_size_read(inode))
		err = map_folio(inode, folio->index, &pbn);
	if (err)
		return err;
	if (!pbn) {
		folio_zero_range(folio, 0, folio_size(folio));
	} else {
		bio = folio_bio(inode, folio, pbn, REQ_OP_READ);
		err = submit_bio_wait(bio);
		bio_put(bio);
		if (err)
			return err;
	}
	folio_mark_uptodate(folio);
	return 0;
}

static int acfs_write_begin(const struct kiocb *iocb,
			    struct address_space *mapping, loff_t pos,
			    unsigned int len, struct folio **foliop,
			    void **fsdata)
{
	struct folio *folio;
	int err;

	folio = __filemap_get_folio(mapping, pos >> PAGE_SHIFT, FGP_WRITEBEGIN,
				    mapping_gfp_mask(mapping));
	if (IS_ERR(folio))
		return PTR_ERR(folio);
	*foliop = folio;
	if (folio_test_uptodate(folio) || len == folio_size(folio))
		return 0;
	err = fill_folio(mapping->host, folio);
	if (err) {
		folio_unlock(folio);
		folio_put(folio);
	}
	return err;
}

static int acfs_write_end(const struct kiocb *iocb,
			  struct address_space *mapping, loff_t pos,
			  unsigned int len, unsigned int copied,
			  struct folio *folio, void *fsdata)
{
	struct inode *inode = mapping->host;

	if (!folio_test_uptodate(folio)) {
		if (copied < len) {
			copied = 0;
			goto out;
		}
		folio_mark_uptodate(folio);
	}
	if (pos + copied > inode->i_size) {
		i_size_write(inode, pos + copied);
		mark_inode_dirty(inode);
	}
	folio_mark_dirty(folio);
out:
	folio_unlock(folio);
	folio_put(folio);
	return copied;
}

static int put_extent(struct acfs_fs *fs, struct acfs_root *root, u64 ino,
		      u64 last, u64 bnr, u32 len)
{
	struct acfs_key key = { ino, ACFS_EXTENT_KEY, last };
	struct acfs_extent e = {
		.bnr = cpu_to_le64(bnr),
		.len = cpu_to_le32(len),
	};

	return acfs_insert_data(fs, root, &key, &e, sizeof(e));
}

/* Folios in one writeback run: one bio's worth, 1 MiB. */
#define RUN_MAX		BIO_MAX_VECS

struct wb_run {
	struct folio *folio[RUN_MAX];
	u64 pbn[RUN_MAX];
	int n;
};

/*
 * Whether one extent maps file blocks [first, first + n) to blocks this
 * transaction wrote and nothing shares, so writeback may overwrite them;
 * then *pbn is the first (lock held).
 */
static int run_in_place(struct acfs_fs *fs, struct inode *inode, u64 first,
			u32 n, u64 *pbn)
{
	struct acfs_key key = { inode->i_ino, ACFS_EXTENT_KEY, first };
	struct acfs_root *root = &ACFS_I(inode)->br->root;
	struct acfs_path path;
	struct acfs_extent e;
	u64 start, bnr, i;
	int ret;
	u32 rc;

	acfs_path_init(&path);
	ret = acfs_search_ge(fs, root, &key, &path, false);
	if (ret)
		return ret < 0 ? ret : 0;
	extent_at(&path, &key, &e);
	acfs_path_release(&path);
	start = key.offset - le32_to_cpu(e.len) + 1;
	if (key.objectid != inode->i_ino || key.type != ACFS_EXTENT_KEY ||
	    start > first || key.offset < first + n - 1)
		return 0;
	bnr = le64_to_cpu(e.bnr) + first - start;
	for (i = 0; i < n; i++) {
		if (!acfs_fresh(fs, bnr + i))
			return 0;
		ret = acfs_rc_get(fs, bnr + i, &rc);
		if (ret)
			return ret;
		if (rc != 1)
			return 0;
	}
	*pbn = bnr;
	return 1;
}

/*
 * Where writeback puts file blocks [first, first + n) (lock held): in
 * place if run_in_place() allows, else new blocks in as few extents as
 * free space allows, starting after the previous file block's (merged
 * into its extent when they follow it). The old blocks lose a reference
 * and stay pinned until the commit if that was their last.
 */
static int map_run(struct acfs_fs *fs, struct inode *inode, u64 first, u32 n,
		   u64 *pbn)
{
	struct acfs_root *root = &ACFS_I(inode)->br->root;
	u64 ino = inode->i_ino, cur = first, bnr = 0, b;
	struct acfs_extent le;
	struct acfs_path path;
	struct acfs_key key;
	u32 left = 0, want, got, i;
	bool grow;
	int ret;

	ret = run_in_place(fs, inode, first, n, &b);
	if (ret) {
		for (i = 0; ret > 0 && i < n; i++)
			pbn[i] = b + i;
		return ret < 0 ? ret : 0;
	}
	ret = acfs_punch(fs, root, inode, ino, first, first + n - 1);
	if (ret)
		return ret;
	if (first) {
		key = (struct acfs_key){ ino, ACFS_EXTENT_KEY, first - 1 };
		ret = acfs_lookup(fs, root, &key, &le, sizeof(le));
		if (ret == sizeof(le)) {
			left = le32_to_cpu(le.len);
			bnr = le64_to_cpu(le.bnr) + left;
		} else if (ret != -ENOENT) {
			return ret < 0 ? ret : -EUCLEAN;
		}
	}
	while (cur < first + n) {
		/* the previous extent can grow if the block after it is free */
		grow = left && left < ACFS_EXTENT_MAX && bnr < fs->nr_blocks &&
		       !test_bit(bnr, fs->used);
		want = min_t(u32, first + n - cur,
			     grow ? ACFS_EXTENT_MAX - left : ACFS_EXTENT_MAX);
		b = bnr;
		ret = acfs_alloc_run(fs, &b, want, &got);
		if (ret)
			return ret;
		for (i = 0; i < got; i++)
			pbn[cur - first + i] = b + i;
		if (grow && b == bnr) {
			/* after the previous extent on disk: grow it */
			key = (struct acfs_key){ ino, ACFS_EXTENT_KEY, cur - 1 };
			acfs_path_init(&path);
			ret = acfs_search(fs, root, &key, &path, 0, true);
			if (ret) {
				acfs_path_release(&path);
				return ret < 0 ? ret : -EUCLEAN;
			}
			ret = acfs_del(fs, root, &path);
			if (!ret)
				ret = put_extent(fs, root, ino, cur + got - 1,
						 bnr - left, left + got);
			left += got;
		} else {
			ret = put_extent(fs, root, ino, cur + got - 1, b, got);
			left = got;
		}
		if (ret)
			return ret;
		inode_add_bytes(inode, (loff_t)got * BS);
		bnr = b + got;
		cur += got;
	}
	return 0;
}

static void write_end_io(struct bio *bio)
{
	struct acfs_fs *fs = bio->bi_private;
	struct folio_iter fi;

	bio_for_each_folio_all(fi, bio) {
		if (bio->bi_status)
			mapping_set_error(fi.folio->mapping,
					  blk_status_to_errno(bio->bi_status));
		folio_end_writeback(fi.folio);
		if (atomic_dec_and_test(&fs->inflight))
			wake_up(&fs->inflight_wq);
	}
	bio_put(bio);
}

/*
 * Write back a run of consecutive locked folios: map them all under one
 * hold of the lock, then send each stretch that is consecutive on disk
 * as one bio.
 */
static int write_run(struct inode *inode, struct wb_run *run,
		     struct writeback_control *wbc)
{
	struct acfs_fs *fs = ACFS_FS(inode->i_sb);
	blk_opf_t opf = REQ_OP_WRITE | wbc_to_write_flags(wbc);
	struct bio *bio = NULL;
	unsigned int nofs;
	loff_t bytes;
	int i, n = run->n, err;

	run->n = 0;
	nofs = acfs_wlock(fs);
	bytes = inode_get_bytes(inode);
	err = fs->error ? -EIO :
	      map_run(fs, inode, run->folio[0]->index, n, run->pbn);
	if (!err && (inode_get_bytes(inode) != bytes ||
		     ACFS_I(inode)->disk_size != i_size_read(inode)))
		err = acfs_write_item(inode);
	if (!err)
		atomic_add(n, &fs->inflight);
	else if (err != -EIO)
		acfs_abort(fs, err);
	acfs_wunlock(fs, nofs);
	for (i = 0; i < n; i++) {
		struct folio *folio = run->folio[i];

		if (err) {
			mapping_set_error(folio->mapping, err);
			folio_unlock(folio);
			continue;
		}
		folio_start_writeback(folio);
		folio_unlock(folio);
		if (bio && (run->pbn[i] != run->pbn[i - 1] + 1 ||
			    !bio_add_folio(bio, folio, folio_size(folio), 0))) {
			submit_bio(bio);
			bio = NULL;
		}
		if (!bio) {
			bio = bio_alloc(inode->i_sb->s_bdev, n - i, opf,
					GFP_NOFS);
			bio->bi_iter.bi_sector = run->pbn[i] <<
				(ACFS_BLOCK_SHIFT - SECTOR_SHIFT);
			bio->bi_private = fs;
			bio->bi_end_io = write_end_io;
			bio_add_folio_nofail(bio, folio, folio_size(folio), 0);
		}
	}
	if (bio)
		submit_bio(bio);
	return err;
}

static int acfs_writepages(struct address_space *mapping,
			   struct writeback_control *wbc)
{
	struct inode *inode = mapping->host;
	struct folio *folio = NULL;
	struct wb_run *run;
	loff_t size, pos;
	int err = 0;

	run = kmalloc(sizeof(*run), GFP_NOFS);
	if (!run)
		return -ENOMEM;
	run->n = 0;
	while ((folio = writeback_iter(mapping, wbc, folio, &err))) {
		size = i_size_read(inode);
		pos = folio_pos(folio);
		if (pos >= size) {
			folio_unlock(folio);
			continue;
		}
		if (size - pos < folio_size(folio))
			folio_zero_segment(folio, size - pos, folio_size(folio));
		if (run->n && (run->n == RUN_MAX ||
			       folio->index != run->folio[run->n - 1]->index + 1)) {
			err = write_run(inode, run, wbc);
			if (err) {
				mapping_set_error(mapping, err);
				folio_unlock(folio);
				continue;
			}
		}
		run->folio[run->n++] = folio;
	}
	if (run->n) {
		int ret = write_run(inode, run, wbc);

		if (!err)
			err = ret;
	}
	kfree(run);
	return err;
}

/* Drop the file's blocks [first, last] (lock held). */
int acfs_punch(struct acfs_fs *fs, struct acfs_root *root, struct inode *inode,
	       u64 ino, u64 first, u64 last)
{
	u64 dropped = 0, start, end, bnr, lo, hi;
	struct acfs_key key;
	struct acfs_path path;
	struct acfs_extent e;
	int ret;

	for (;;) {
		key = (struct acfs_key){ ino, ACFS_EXTENT_KEY, first };
		acfs_path_init(&path);
		ret = acfs_search_ge(fs, root, &key, &path, true);
		if (ret)
			break;
		extent_at(&path, &key, &e);
		bnr = le64_to_cpu(e.bnr);
		end = key.offset;
		start = end - le32_to_cpu(e.len) + 1;
		if (key.objectid != ino || key.type != ACFS_EXTENT_KEY ||
		    start > last) {
			acfs_path_release(&path);
			break;
		}
		ret = acfs_del(fs, root, &path);
		lo = max(start, first);
		hi = min(end, last);
		if (!ret && start < first)
			ret = put_extent(fs, root, ino, first - 1, bnr,
					 first - start);
		if (!ret && end > last)
			ret = put_extent(fs, root, ino, end,
					 bnr + last + 1 - start, end - last);
		if (!ret)
			ret = acfs_rc_add(fs, bnr + lo - start, hi - lo + 1, -1);
		if (ret)
			break;
		dropped += hi - lo + 1;
	}
	if (inode)
		inode_sub_bytes(inode, dropped << ACFS_BLOCK_SHIFT);
	return ret < 0 ? ret : 0;
}

/* Zero the rest of the last block after a truncate, if it has a block. */
static int zero_tail(struct inode *inode, loff_t size)
{
	struct folio *folio;
	u64 pbn;
	int err;

	err = map_folio(inode, size >> PAGE_SHIFT, &pbn);
	if (err || !pbn)
		return err;
	folio = read_mapping_folio(inode->i_mapping, size >> PAGE_SHIFT, NULL);
	if (IS_ERR(folio))
		return PTR_ERR(folio);
	folio_lock(folio);
	if (folio->mapping == inode->i_mapping) {
		folio_zero_segment(folio, offset_in_folio(folio, size),
				   folio_size(folio));
		folio_mark_dirty(folio);
	}
	folio_unlock(folio);
	folio_put(folio);
	return 0;
}

/* After truncate_setsize() from old_size (i_rwsem held). */
int acfs_truncate(struct inode *inode, loff_t old_size)
{
	struct acfs_fs *fs = ACFS_FS(inode->i_sb);
	loff_t size = inode->i_size;
	unsigned int nofs;
	int err;

	if (size >= old_size)
		return 0;
	nofs = acfs_wlock(fs);
	err = fs->error ? -EIO :
	      acfs_punch(fs, &ACFS_I(inode)->br->root, inode, inode->i_ino,
			 DIV_ROUND_UP(size, BS), U64_MAX);
	if (!err)
		err = acfs_write_item(inode);
	if (err && err != -EIO)
		acfs_abort(fs, err);
	acfs_wunlock(fs, nofs);
	if (!err && (size & (BS - 1)))
		err = zero_tail(inode, size);
	return err;
}

/*
 * The file's changes are in the running transaction or older ones once
 * its data is written back and its item updated: commit that one, unless
 * a commit since (another fsync's, the periodic one) has.
 */
int acfs_fsync(struct file *file, loff_t start, loff_t end, int datasync)
{
	struct inode *inode = file->f_mapping->host;
	struct acfs_fs *fs = ACFS_FS(inode->i_sb);
	unsigned int nofs;
	u64 gen;
	int err;

	err = file_write_and_wait_range(file, start, end);
	if (err)
		return err;
	if (READ_ONCE(ACFS_I(inode)->item_dirty) && !acfs_ro(inode)) {
		nofs = acfs_wlock(fs);
		err = fs->error ? -EIO : acfs_write_item(inode);
		gen = fs->gen;
		acfs_wunlock(fs, nofs);
		if (err)
			return err;
	} else {
		/* writeback mapped the data under the lock, in gen or before */
		gen = READ_ONCE(fs->gen);
	}
	return acfs_commit_gen(fs, gen);
}

struct remap_ext {
	u64 lblk;
	u64 bnr;
	u32 len;
};

#define REMAP_BATCH	64

/* Collect up to REMAP_BATCH pieces of src's blocks [*cur, last]. */
static int remap_collect(struct acfs_fs *fs, struct inode *src, u64 *cur,
			 u64 last, struct remap_ext *ext, int *n)
{
	struct acfs_root *root = &ACFS_I(src)->br->root;
	struct acfs_path path;
	struct acfs_extent e;
	struct acfs_key key;
	u64 start, lo;
	int ret;

	*n = 0;
	while (*n < REMAP_BATCH && *cur <= last) {
		key = (struct acfs_key){ src->i_ino, ACFS_EXTENT_KEY, *cur };
		acfs_path_init(&path);
		ret = acfs_search_ge(fs, root, &key, &path, false);
		if (ret)
			return ret < 0 ? ret : 0;
		extent_at(&path, &key, &e);
		acfs_path_release(&path);
		start = key.offset - le32_to_cpu(e.len) + 1;
		if (key.objectid != src->i_ino || key.type != ACFS_EXTENT_KEY ||
		    start > last)
			return 0;
		lo = max(start, *cur);
		ext[*n].lblk = lo;
		ext[*n].bnr = le64_to_cpu(e.bnr) + lo - start;
		ext[*n].len = min(key.offset, last) - lo + 1;
		(*n)++;
		*cur = key.offset + 1;
	}
	return 0;
}

/* Make dst's blocks [dfirst, dfirst + n) share src's [sfirst, sfirst + n). */
static int remap_blocks(struct acfs_fs *fs, struct inode *src, u64 sfirst,
			struct inode *dst, u64 dfirst, u64 n, loff_t end)
{
	struct acfs_root *droot = &ACFS_I(dst)->br->root;
	u64 cur = sfirst, last = sfirst + n - 1;
	struct remap_ext *ext;
	unsigned int nofs;
	int i, got, err;

	ext = kmalloc_array(REMAP_BATCH, sizeof(*ext), GFP_KERNEL);
	if (!ext)
		return -ENOMEM;
	nofs = acfs_wlock(fs);
	err = fs->error ? -EIO :
	      acfs_punch(fs, droot, dst, dst->i_ino, dfirst, dfirst + n - 1);
	while (!err && cur <= last) {
		err = remap_collect(fs, src, &cur, last, ext, &got);
		for (i = 0; !err && i < got; i++) {
			err = put_extent(fs, droot, dst->i_ino,
					 dfirst + ext[i].lblk - sfirst +
					 ext[i].len - 1, ext[i].bnr,
					 ext[i].len);
			if (!err)
				err = acfs_rc_add(fs, ext[i].bnr, ext[i].len, 1);
			if (!err)
				inode_add_bytes(dst, (loff_t)ext[i].len * BS);
		}
		if (got < REMAP_BATCH)
			break;
	}
	if (!err) {
		if (end > i_size_read(dst))
			i_size_write(dst, end);
		inode_set_mtime_to_ts(dst, inode_set_ctime_current(dst));
		err = acfs_write_item(dst);
	}
	if (err && err != -EIO)
		acfs_abort(fs, err);
	acfs_wunlock(fs, nofs);
	kfree(ext);
	return err;
}

/*
 * Clone or dedupe a range: the destination then refers to the source's
 * blocks. Works across branches (copy_file_range does; FICLONE wants
 * one mount), so a branch takes data from another without copying.
 */
static loff_t acfs_remap_file_range(struct file *file_in, loff_t pos_in,
				    struct file *file_out, loff_t pos_out,
				    loff_t len, unsigned int remap_flags)
{
	struct inode *src = file_inode(file_in), *dst = file_inode(file_out);
	struct acfs_fs *fs = ACFS_FS(dst->i_sb);
	loff_t ret;

	if (remap_flags & ~(REMAP_FILE_DEDUP | REMAP_FILE_ADVISORY))
		return -EINVAL;
	if (acfs_ro(dst))
		return -EROFS;
	ret = acfs_reserve_check(fs);
	if (ret)
		return ret;
	lock_two_nondirectories(src, dst);
	filemap_invalidate_lock_two(src->i_mapping, dst->i_mapping);
	ret = generic_remap_file_range_prep(file_in, pos_in, file_out, pos_out,
					    &len, remap_flags);
	if (ret || !len)
		goto out;
	ret = remap_blocks(fs, src, pos_in >> ACFS_BLOCK_SHIFT, dst,
			   pos_out >> ACFS_BLOCK_SHIFT, DIV_ROUND_UP(len, BS),
			   pos_out + len);
	truncate_inode_pages_range(&dst->i_data, pos_out,
				   round_up(pos_out + len, BS) - 1);
out:
	filemap_invalidate_unlock_two(src->i_mapping, dst->i_mapping);
	unlock_two_nondirectories(src, dst);
	return ret < 0 ? ret : len;
}

static ssize_t acfs_write_iter(struct kiocb *iocb, struct iov_iter *from)
{
	int err = acfs_reserve_check(ACFS_FS(file_inode(iocb->ki_filp)->i_sb));

	if (err)
		return err;
	return generic_file_write_iter(iocb, from);
}

const struct address_space_operations acfs_aops = {
	.read_folio	= acfs_read_folio,
	.writepages	= acfs_writepages,
	.dirty_folio	= filemap_dirty_folio,
	.write_begin	= acfs_write_begin,
	.write_end	= acfs_write_end,
	.migrate_folio	= filemap_migrate_folio,
	.error_remove_folio = generic_error_remove_folio,
};

const struct file_operations acfs_file_operations = {
	.llseek		= generic_file_llseek,
	.read_iter	= generic_file_read_iter,
	.write_iter	= acfs_write_iter,
	.mmap_prepare	= generic_file_mmap_prepare,
	.open		= generic_file_open,
	.fsync		= acfs_fsync,
	.splice_read	= filemap_splice_read,
	.splice_write	= iter_file_splice_write,
	.remap_file_range = acfs_remap_file_range,
	.unlocked_ioctl	= acfs_ioctl,
	.compat_ioctl	= compat_ptr_ioctl,
};

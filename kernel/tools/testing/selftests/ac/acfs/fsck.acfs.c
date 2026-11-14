// SPDX-License-Identifier: GPL-2.0
/*
 * fsck.acfs DEVICE: check an unmounted acfs file system, read-only.
 *
 * Recounts every block's references the way the kernel keeps them (lazy:
 * one per parent node, extent item or branch item, however many trees
 * share the parent) and compares with the refcount table; checks node
 * checksums and key order; and checks each branch's files: entries and
 * their index items agree, link counts, directory sizes and parents,
 * extents that do not overlap and add up to the inode's blocks.
 * Exits 0 when clean, 1 with what is wrong.
 */
#define _GNU_SOURCE
#include <endian.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/stat.h>
#include <unistd.h>
#include <linux/fs.h>
#include <linux/ac/acfs.h>
#include <linux/ac/acfs_tree.h>

#define BS	ACFS_BLOCK_SIZE
#define HDR	sizeof(struct acfs_node_header)

static int fd, errors;
static uint64_t stat_branches, stat_inodes, stat_blocks;
static uint64_t nr, rc_pages, data_start, committed, next_ino;
static uint32_t *table, *counted;
static uint8_t *visited;
static uint32_t crc_table[256];

#define BAD(...) do {							\
	if (errors++ < 30) {						\
		printf("fsck.acfs: ");					\
		printf(__VA_ARGS__);					\
		printf("\n");						\
	}								\
} while (0)

static uint32_t csum(const uint8_t *b)
{
	uint32_t crc = ~0U;

	for (size_t i = 4; i < BS; i++)
		crc = crc_table[(crc ^ b[i]) & 0xff] ^ (crc >> 8);
	return crc;
}

static void get(uint64_t bnr, uint8_t *b)
{
	if (pread(fd, b, BS, bnr * BS) != BS) {
		perror("fsck.acfs: read");
		exit(2);
	}
}

struct key {
	uint64_t objectid;
	uint8_t type;
	uint64_t offset;
};

static struct key dkey(const struct acfs_disk_key *d)
{
	return (struct key){ le64toh(d->objectid), d->type, le64toh(d->offset) };
}

static int kcmp(struct key a, struct key b)
{
	if (a.objectid != b.objectid)
		return a.objectid < b.objectid ? -1 : 1;
	if (a.type != b.type)
		return a.type < b.type ? -1 : 1;
	if (a.offset != b.offset)
		return a.offset < b.offset ? -1 : 1;
	return 0;
}

static void count(uint64_t bnr, uint64_t len, const char *what)
{
	if (bnr < data_start || bnr >= nr || len > nr - bnr) {
		BAD("%s points outside the disk: %llu+%llu", what,
		    (unsigned long long)bnr, (unsigned long long)len);
		return;
	}
	for (uint64_t i = 0; i < len; i++)
		counted[bnr + i]++;
}

static int node_ok(uint8_t *b, uint64_t bnr)
{
	struct acfs_node_header *h = (void *)b;
	uint32_t n = le16toh(h->nritems), end = BS;

	if (le32toh(h->magic) != ACFS_NODE_MAGIC || le64toh(h->bnr) != bnr ||
	    le32toh(h->csum) != csum(b) || h->level >= ACFS_MAX_LEVEL) {
		BAD("node %llu: bad header or checksum", (unsigned long long)bnr);
		return 0;
	}
	if (h->level)
		return n >= 1 && n <= (BS - HDR) / sizeof(struct acfs_key_ptr);
	for (uint32_t i = 0; i < n; i++) {
		struct acfs_item *it = (void *)(b + HDR + i * sizeof(*it));
		uint32_t off = le16toh(it->offset), size = le16toh(it->size);

		if (off < HDR + n * sizeof(*it) || off + size != end) {
			BAD("node %llu: item %u misplaced", (unsigned long long)bnr, i);
			return 0;
		}
		end = off;
	}
	return 1;
}

/* Count references below a node the first time it is reached. */
static void walk(uint64_t bnr, int level, const char *what)
{
	uint8_t b[BS];
	struct acfs_node_header *h = (void *)b;
	struct key prev = {}, k;
	uint32_t n;

	count(bnr, 1, what);
	if (bnr < data_start || bnr >= nr || visited[bnr])
		return;
	visited[bnr] = 1;
	get(bnr, b);
	if (!node_ok(b, bnr))
		return;
	if (level >= 0 && h->level != level)
		BAD("node %llu: level %d, want %d", (unsigned long long)bnr,
		    h->level, level);
	n = le16toh(h->nritems);
	for (uint32_t i = 0; i < n; i++) {
		if (h->level) {
			struct acfs_key_ptr *p = (void *)(b + HDR + i * sizeof(*p));

			k = dkey(&p->key);
			if (i > 1 && kcmp(prev, k) >= 0)
				BAD("node %llu: keys out of order", (unsigned long long)bnr);
			walk(le64toh(p->bnr), h->level - 1, what);
		} else {
			struct acfs_item *it = (void *)(b + HDR + i * sizeof(*it));

			k = dkey(&it->key);
			if (i && kcmp(prev, k) >= 0)
				BAD("leaf %llu: keys out of order", (unsigned long long)bnr);
			if (k.type == ACFS_EXTENT_KEY) {
				struct acfs_extent *e = (void *)(b + le16toh(it->offset));

				count(le64toh(e->bnr), le32toh(e->len), "extent");
			}
		}
		prev = k;
	}
}

/* Every item of a tree in key order. */
struct item {
	struct key key;
	uint32_t size;
	uint8_t *data;
};

static struct item *items;
static size_t nitems, capitems;

static void collect(uint64_t bnr)
{
	uint8_t *b = malloc(BS);
	struct acfs_node_header *h = (void *)b;
	uint32_t n;

	get(bnr, b);
	if (!node_ok(b, bnr)) {
		free(b);
		return;
	}
	n = le16toh(h->nritems);
	for (uint32_t i = 0; i < n; i++) {
		if (h->level) {
			struct acfs_key_ptr *p = (void *)(b + HDR + i * sizeof(*p));

			collect(le64toh(p->bnr));
			continue;
		}
		struct acfs_item *it = (void *)(b + HDR + i * sizeof(*it));

		if (nitems == capitems) {
			capitems = capitems ? capitems * 2 : 1024;
			items = realloc(items, capitems * sizeof(*items));
		}
		items[nitems].key = dkey(&it->key);
		items[nitems].size = le16toh(it->size);
		items[nitems].data = malloc(items[nitems].size + 1);
		memcpy(items[nitems].data, b + le16toh(it->offset),
		       items[nitems].size);
		nitems++;
	}
	free(b);
}

static void free_items(void)
{
	for (size_t i = 0; i < nitems; i++)
		free(items[i].data);
	nitems = 0;
}

static uint64_t xxh64(const void *p, size_t len);

struct ino {
	uint64_t ino, nlink, size, parent, dir_seq, blocks;
	uint32_t mode;
	uint64_t links, subdirs, names, extent_blocks, max_index, last_end;
	int orphan;
};

static struct ino *inos;
static size_t ninos;

static struct ino *find_ino(uint64_t ino)
{
	size_t lo = 0, hi = ninos;

	while (lo < hi) {
		size_t mid = (lo + hi) / 2;

		if (inos[mid].ino < ino)
			lo = mid + 1;
		else
			hi = mid;
	}
	return lo < ninos && inos[lo].ino == ino ? &inos[lo] : NULL;
}

static void check_branch(uint64_t id, uint64_t root)
{
	size_t i;

	nitems = 0;
	collect(root);
	free(inos);
	inos = calloc(nitems + 1, sizeof(*inos));
	ninos = 0;
	for (i = 0; i < nitems; i++) {
		struct item *it = &items[i];
		struct acfs_inode_item ii;

		if (i && kcmp(items[i - 1].key, it->key) >= 0)
			BAD("branch %llu: keys out of order across leaves",
			    (unsigned long long)id);
		if (it->key.type != ACFS_INODE_KEY)
			continue;
		if (it->size != sizeof(ii)) {
			BAD("branch %llu: inode %llu item size %u",
			    (unsigned long long)id,
			    (unsigned long long)it->key.objectid, it->size);
			continue;
		}
		memcpy(&ii, it->data, sizeof(ii));
		stat_inodes++;
		inos[ninos++] = (struct ino){
			.ino = it->key.objectid, .nlink = le32toh(ii.nlink),
			.size = le64toh(ii.size), .parent = le64toh(ii.parent),
			.dir_seq = le64toh(ii.dir_seq),
			.blocks = le64toh(ii.blocks), .mode = le32toh(ii.mode),
		};
		if (it->key.objectid >= next_ino && it->key.objectid != ACFS_ROOT_INO)
			BAD("branch %llu: inode %llu not below next_ino",
			    (unsigned long long)id,
			    (unsigned long long)it->key.objectid);
	}
	for (i = 0; i < nitems; i++) {
		struct item *it = &items[i];
		struct ino *in = find_ino(it->key.objectid), *child;
		uint32_t off = 0;

		switch (it->key.type) {
		case ACFS_ORPHAN_KEY:
			if (it->key.objectid != ACFS_ORPHAN_OBJID)
				BAD("orphan item under %llu",
				    (unsigned long long)it->key.objectid);
			child = find_ino(it->key.offset);
			if (child)
				child->orphan = 1;
			continue;
		case ACFS_INODE_KEY:
			continue;
		}
		if (!in) {
			BAD("branch %llu: item (%llu %u %llu) without inode",
			    (unsigned long long)id,
			    (unsigned long long)it->key.objectid, it->key.type,
			    (unsigned long long)it->key.offset);
			continue;
		}
		switch (it->key.type) {
		case ACFS_DIRENT_KEY:
			while (off + sizeof(struct acfs_dirent) <= it->size) {
				struct acfs_dirent de;
				uint16_t nl;

				memcpy(&de, it->data + off, sizeof(de));
				nl = le16toh(de.name_len);
				if (off + sizeof(de) + nl > it->size)
					break;
				if (xxh64(it->data + off + sizeof(de), nl) !=
				    it->key.offset)
					BAD("dir %llu: entry under the wrong hash",
					    (unsigned long long)in->ino);
				child = find_ino(le64toh(de.ino));
				if (!child) {
					BAD("dir %llu: entry %.*s names missing inode %llu",
					    (unsigned long long)in->ino, nl,
					    it->data + off + sizeof(de),
					    (unsigned long long)le64toh(de.ino));
				} else {
					child->links++;
					if (S_ISDIR(child->mode)) {
						in->subdirs++;
						if (child->parent != in->ino)
							BAD("dir %llu: parent %llu, entry in %llu",
							    (unsigned long long)child->ino,
							    (unsigned long long)child->parent,
							    (unsigned long long)in->ino);
					}
					if (de.type != ((child->mode >> 12) & 15))
						BAD("dir %llu: entry type", (unsigned long long)in->ino);
				}
				in->names += nl;
				if (le64toh(de.index) > in->max_index)
					in->max_index = le64toh(de.index);
				{
					struct key k = { in->ino, ACFS_DIRINDEX_KEY,
							 le64toh(de.index) };
					size_t lo = 0, hi = nitems;

					while (lo < hi) {
						size_t mid = (lo + hi) / 2;

						if (kcmp(items[mid].key, k) < 0)
							lo = mid + 1;
						else
							hi = mid;
					}
					if (lo == nitems || kcmp(items[lo].key, k) ||
					    items[lo].size != sizeof(de) + nl ||
					    memcmp(items[lo].data, it->data + off,
						   sizeof(de) + nl))
						BAD("dir %llu: entry without its index item",
						    (unsigned long long)in->ino);
				}
				off += sizeof(de) + nl;
			}
			if (off != it->size)
				BAD("dir %llu: entry item damaged",
				    (unsigned long long)in->ino);
			break;
		case ACFS_EXTENT_KEY: {
			struct acfs_extent e;
			uint64_t len, start;

			memcpy(&e, it->data, sizeof(e));
			len = le32toh(e.len);
			start = it->key.offset - len + 1;
			if (!len || len > ACFS_EXTENT_MAX)
				BAD("inode %llu: extent length %llu",
				    (unsigned long long)in->ino,
				    (unsigned long long)len);
			if (in->extent_blocks && start < in->last_end)
				BAD("inode %llu: extents overlap at %llu",
				    (unsigned long long)in->ino,
				    (unsigned long long)start);
			in->last_end = it->key.offset + 1;
			in->extent_blocks += len;
			break;
		}
		}
	}
	for (i = 0; i < nitems; i++)
		if (items[i].key.type == ACFS_DIRINDEX_KEY) {
			struct ino *in = find_ino(items[i].key.objectid);

			if (in && items[i].key.offset > in->max_index)
				BAD("dir %llu: index item without entry",
				    (unsigned long long)in->ino);
		}
	for (i = 0; i < ninos; i++) {
		struct ino *in = &inos[i];
		uint64_t want = in->links;

		if (S_ISDIR(in->mode)) {
			want = in->ino == ACFS_ROOT_INO || in->links ?
			       2 + in->subdirs : 0;
			if (in->size != in->names)
				BAD("dir %llu: size %llu, names %llu",
				    (unsigned long long)in->ino,
				    (unsigned long long)in->size,
				    (unsigned long long)in->names);
			if (in->names && in->dir_seq <= in->max_index)
				BAD("dir %llu: dir_seq behind",
				    (unsigned long long)in->ino);
			if (in->links > 1)
				BAD("dir %llu: %llu entries", (unsigned long long)in->ino,
				    (unsigned long long)in->links);
		}
		if (in->nlink != want && !(in->orphan && !in->links))
			BAD("branch %llu inode %llu: nlink %llu, %llu entries",
			    (unsigned long long)id, (unsigned long long)in->ino,
			    (unsigned long long)in->nlink,
			    (unsigned long long)want);
		if (!in->links && in->ino != ACFS_ROOT_INO && !in->orphan)
			BAD("branch %llu inode %llu: lost", (unsigned long long)id,
			    (unsigned long long)in->ino);
		if (in->blocks != in->extent_blocks)
			BAD("inode %llu: blocks %llu, extents %llu",
			    (unsigned long long)in->ino,
			    (unsigned long long)in->blocks,
			    (unsigned long long)in->extent_blocks);
		if (S_ISREG(in->mode) && in->extent_blocks &&
		    in->last_end > (in->size + BS - 1) / BS)
			BAD("inode %llu: extent past the end",
			    (unsigned long long)in->ino);
	}
	if (!find_ino(ACFS_ROOT_INO))
		BAD("branch %llu: no root directory", (unsigned long long)id);
	free_items();
}

#define P1 11400714785074694791ULL
#define P2 14029467366897019727ULL
#define P3 1609587929392839161ULL
#define P4 9650029242287828579ULL
#define P5 2870177450012600261ULL

static uint64_t rotl(uint64_t x, int r)
{
	return (x << r) | (x >> (64 - r));
}

static uint64_t round1(uint64_t acc, uint64_t in)
{
	return rotl(acc + in * P2, 31) * P1;
}

static uint64_t merge(uint64_t acc, uint64_t v)
{
	return (acc ^ round1(0, v)) * P1 + P4;
}

static uint64_t xxh64(const void *in, size_t len)
{
	const uint8_t *p = in, *end = p + len;
	uint64_t h;

	if (len >= 32) {
		uint64_t v1 = P1 + P2, v2 = P2, v3 = 0, v4 = -P1;

		do {
			uint64_t w[4];

			memcpy(w, p, 32);
			v1 = round1(v1, le64toh(w[0]));
			v2 = round1(v2, le64toh(w[1]));
			v3 = round1(v3, le64toh(w[2]));
			v4 = round1(v4, le64toh(w[3]));
			p += 32;
		} while (p + 32 <= end);
		h = rotl(v1, 1) + rotl(v2, 7) + rotl(v3, 12) + rotl(v4, 18);
		h = merge(h, v1);
		h = merge(h, v2);
		h = merge(h, v3);
		h = merge(h, v4);
	} else {
		h = P5;
	}
	h += len;
	while (p + 8 <= end) {
		uint64_t w;

		memcpy(&w, p, 8);
		h ^= round1(0, le64toh(w));
		h = rotl(h, 27) * P1 + P4;
		p += 8;
	}
	if (p + 4 <= end) {
		uint32_t w;

		memcpy(&w, p, 4);
		h ^= (uint64_t)le32toh(w) * P1;
		h = rotl(h, 23) * P2 + P3;
		p += 4;
	}
	while (p < end) {
		h ^= *p++ * P5;
		h = rotl(h, 11) * P1;
	}
	h ^= h >> 33;
	h *= P2;
	h ^= h >> 29;
	h *= P3;
	h ^= h >> 32;
	return h;
}

int main(int argc, char **argv)
{
	uint8_t b[BS];
	struct acfs_super s;
	uint64_t root_tree = 0, best = 0, mismatches = 0;
	struct stat st;
	int found = 0;

	if (argc != 2) {
		fprintf(stderr, "usage: fsck.acfs DEVICE\n");
		return 2;
	}
	for (uint32_t i = 0; i < 256; i++) {
		uint32_t c = i;

		for (int k = 0; k < 8; k++)
			c = c & 1 ? (c >> 1) ^ 0x82F63B78 : c >> 1;
		crc_table[i] = c;
	}
	fd = open(argv[1], O_RDONLY);
	if (fd < 0 || fstat(fd, &st)) {
		perror(argv[1]);
		return 2;
	}
	for (int slot = 0; slot < ACFS_SB_SLOTS; slot++) {
		struct acfs_super *d = (void *)b;

		get(slot, b);
		if (le32toh(d->magic) == ACFS_SUPER_MAGIC &&
		    le32toh(d->csum) == csum(b) &&
		    (!found || le64toh(d->gen) > best)) {
			memcpy(&s, d, sizeof(s));
			best = le64toh(d->gen);
			found = 1;
		}
	}
	if (!found) {
		printf("fsck.acfs: no valid superblock\n");
		return 1;
	}
	committed = best;
	nr = le64toh(s.nr_blocks);
	rc_pages = le64toh(s.rc_pages);
	data_start = ACFS_RC_START + 2 * rc_pages;
	root_tree = le64toh(s.root_tree);
	next_ino = le64toh(s.next_ino);
	table = calloc(nr, 4);
	counted = calloc(nr, 4);
	visited = calloc(nr, 1);
	for (uint64_t p = 0; p < rc_pages; p++) {
		uint8_t c[2][BS];
		int pick = -1;
		uint64_t gen = 0;

		for (int slot = 0; slot < 2; slot++) {
			struct acfs_rc_page *pg = (void *)c[slot];

			get(ACFS_RC_START + 2 * p + slot, c[slot]);
			if (le32toh(pg->magic) == ACFS_RC_MAGIC &&
			    le64toh(pg->page) == p && le32toh(pg->csum) == csum(c[slot]) &&
			    le64toh(pg->gen) <= committed &&
			    (pick < 0 || le64toh(pg->gen) > gen)) {
				pick = slot;
				gen = le64toh(pg->gen);
			}
		}
		if (pick < 0) {
			BAD("refcount page %llu: no valid copy", (unsigned long long)p);
			continue;
		}
		for (uint64_t i = 0; i < ACFS_RC_PER_PAGE &&
		     p * ACFS_RC_PER_PAGE + i < nr; i++)
			table[p * ACFS_RC_PER_PAGE + i] =
				le32toh(((struct acfs_rc_page *)c[pick])->count[i]);
	}
	for (uint64_t i = 0; i < data_start; i++)
		counted[i] = 1;

	walk(root_tree, -1, "root tree");
	nitems = 0;
	collect(root_tree);
	{
		size_t n = nitems;
		struct item *branches = items;

		items = NULL;
		capitems = 0;
		nitems = 0;
		for (size_t i = 0; i < n; i++) {
			struct acfs_branch_item bi;

			if (branches[i].key.type != ACFS_BRANCH_KEY ||
			    branches[i].size != sizeof(bi)) {
				BAD("root tree: odd item");
				continue;
			}
			memcpy(&bi, branches[i].data, sizeof(bi));
			walk(le64toh(bi.root), -1, "branch");
			stat_branches++;
			if (!(le64toh(bi.flags) & ACFS_BRANCH_DELETED))
				check_branch(branches[i].key.objectid,
					     le64toh(bi.root));
			if (branches[i].key.objectid >= le64toh(s.next_branch))
				BAD("branch %llu not below next_branch",
				    (unsigned long long)branches[i].key.objectid);
		}
	}
	for (uint64_t i = 0; i < nr; i++)
		stat_blocks += !!counted[i];
	for (uint64_t i = 0; i < nr; i++)
		if (table[i] != counted[i]) {
			if (mismatches++ < 10)
				BAD("block %llu: refcount %u, %u references",
				    (unsigned long long)i, table[i], counted[i]);
		}
	if (mismatches)
		printf("fsck.acfs: %llu blocks with the wrong refcount\n",
		       (unsigned long long)mismatches);
	if (errors)
		return 1;
	printf("fsck.acfs: %s clean, generation %llu: %llu branches, %llu inodes, %llu blocks used\n",
	       argv[1], (unsigned long long)committed,
	       (unsigned long long)stat_branches,
	       (unsigned long long)stat_inodes,
	       (unsigned long long)stat_blocks);
	return 0;
}

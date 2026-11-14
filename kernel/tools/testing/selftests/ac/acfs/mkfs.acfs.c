// SPDX-License-Identifier: GPL-2.0
/*
 * mkfs.acfs DEVICE: an empty acfs file system with a main branch whose
 * root directory is root's, mode 0755.
 */
#define _GNU_SOURCE
#include <endian.h>
#include <errno.h>
#include <fcntl.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/random.h>
#include <sys/stat.h>
#include <time.h>
#include <unistd.h>
#include <linux/fs.h>
#include <linux/ac/acfs_tree.h>

#define BS ACFS_BLOCK_SIZE

static uint32_t crc_table[256];

static void crc_init(void)
{
	for (uint32_t i = 0; i < 256; i++) {
		uint32_t c = i;

		for (int k = 0; k < 8; k++)
			c = c & 1 ? (c >> 1) ^ 0x82F63B78 : c >> 1;
		crc_table[i] = c;
	}
}

static uint32_t csum(const void *block)
{
	const uint8_t *p = (const uint8_t *)block + 4;
	uint32_t crc = ~0U;

	for (size_t i = 4; i < BS; i++)
		crc = crc_table[(crc ^ *p++) & 0xff] ^ (crc >> 8);
	return crc;
}

static void put(int fd, uint64_t bnr, const void *block)
{
	if (pwrite(fd, block, BS, bnr * BS) != BS) {
		perror("mkfs.acfs: write");
		exit(1);
	}
}

static void leaf(uint8_t *b, uint64_t bnr, uint64_t owner,
		 const struct acfs_disk_key *key, const void *data,
		 uint16_t size)
{
	struct acfs_node_header *h = (void *)b;
	struct acfs_item *it = (void *)(b + sizeof(*h));

	memset(b, 0, BS);
	h->magic = htole32(ACFS_NODE_MAGIC);
	h->bnr = htole64(bnr);
	h->gen = htole64(1);
	h->owner = htole64(owner);
	h->nritems = htole16(1);
	it->key = *key;
	it->offset = htole16(BS - size);
	it->size = htole16(size);
	memcpy(b + BS - size, data, size);
	h->csum = htole32(csum(b));
}

int main(int argc, char **argv)
{
	uint64_t size, nr, rc_pages, data_start, root_tree, fs_root;
	struct acfs_branch_item bi = {};
	struct acfs_inode_item ii = {};
	struct acfs_disk_key key;
	struct acfs_super *s;
	struct stat st;
	uint8_t *b;
	int fd;

	if (argc != 2) {
		fprintf(stderr, "usage: mkfs.acfs DEVICE\n");
		return 2;
	}
	fd = open(argv[1], O_RDWR);
	if (fd < 0 || fstat(fd, &st)) {
		perror(argv[1]);
		return 1;
	}
	if (S_ISBLK(st.st_mode)) {
		if (ioctl(fd, BLKGETSIZE64, &size)) {
			perror("BLKGETSIZE64");
			return 1;
		}
	} else {
		size = st.st_size;
	}
	nr = size / BS;
	rc_pages = (nr + ACFS_RC_PER_PAGE - 1) / ACFS_RC_PER_PAGE;
	data_start = ACFS_RC_START + 2 * rc_pages;
	if (nr < data_start + 64) {
		fprintf(stderr, "mkfs.acfs: %s is too small\n", argv[1]);
		return 1;
	}
	root_tree = data_start;
	fs_root = data_start + 1;
	crc_init();
	b = calloc(1, BS);

	put(fd, 0, b);
	for (uint64_t p = 0; p < rc_pages; p++) {
		struct acfs_rc_page *pg = (void *)b;

		memset(b, 0, BS);
		put(fd, ACFS_RC_START + 2 * p + 1, b);
		pg->magic = htole32(ACFS_RC_MAGIC);
		pg->gen = htole64(1);
		pg->page = htole64(p);
		for (uint64_t i = 0; i < ACFS_RC_PER_PAGE; i++)
			if (p * ACFS_RC_PER_PAGE + i < fs_root + 1)
				pg->count[i] = htole32(1);
		pg->csum = htole32(csum(b));
		put(fd, ACFS_RC_START + 2 * p, b);
	}

	bi.root = htole64(fs_root);
	bi.gen = htole64(1);
	bi.ctime.sec = htole64(time(NULL));
	bi.name_len = 4;
	memcpy(bi.name, "main", 4);
	key = (struct acfs_disk_key){ htole64(ACFS_MAIN_BRANCH),
				      ACFS_BRANCH_KEY, 0 };
	leaf(b, root_tree, 0, &key, &bi, sizeof(bi));
	put(fd, root_tree, b);

	ii.parent = htole64(ACFS_ROOT_INO);
	ii.dir_seq = htole64(2);
	ii.mode = htole32(S_IFDIR | 0755);
	ii.nlink = htole32(2);
	ii.atime.sec = ii.mtime.sec = ii.ctime.sec = ii.btime.sec =
		htole64(time(NULL));
	key = (struct acfs_disk_key){ htole64(ACFS_ROOT_INO), ACFS_INODE_KEY, 0 };
	leaf(b, fs_root, ACFS_MAIN_BRANCH, &key, &ii, sizeof(ii));
	put(fd, fs_root, b);

	memset(b, 0, BS);
	put(fd, 0, b);
	s = (void *)b;
	s->magic = htole32(ACFS_SUPER_MAGIC);
	s->version = htole32(ACFS_VERSION);
	s->block_size = htole32(BS);
	s->gen = htole64(1);
	s->nr_blocks = htole64(nr);
	s->rc_pages = htole64(rc_pages);
	s->root_tree = htole64(root_tree);
	s->next_ino = htole64(ACFS_FIRST_INO);
	s->next_branch = htole64(ACFS_MAIN_BRANCH + 1);
	s->free_blocks = htole64(nr - fs_root - 1);
	if (getrandom(s->uuid, sizeof(s->uuid), 0) != sizeof(s->uuid))
		memset(s->uuid, 0xac, sizeof(s->uuid));
	s->csum = htole32(csum(b));
	put(fd, 1, b);
	if (fsync(fd)) {
		perror("fsync");
		return 1;
	}
	printf("%s: acfs, %llu blocks of %u bytes\n", argv[1],
	       (unsigned long long)nr, BS);
	return 0;
}

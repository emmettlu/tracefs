/* SPDX-License-Identifier: GPL-2.0 */
/*
 * Random operations on a tree, mirrored on another (tmpfs), for
 * acfs_stress and acfs_concurrent. Not thread-safe: one per process.
 */
#ifndef ACFS_OPS_H
#define ACFS_OPS_H

#include "acfs_util.h"

#define MAXENT	400

static uint64_t rng;

static inline uint64_t rnd(void)
{
	rng ^= rng << 13;
	rng ^= rng >> 7;
	rng ^= rng << 17;
	return rng;
}

static inline uint64_t below(uint64_t n)
{
	return n ? rnd() % n : 0;
}

/* Entries of the reference tree, relative paths ("" is the root). */
static char ents[MAXENT + 1][256];
static int types[MAXENT + 1], nents;

static inline void scan(const char *root, const char *rel)
{
	char p[512];
	struct dirent *e;
	DIR *d;

	snprintf(p, sizeof(p), "%s/%s", root, rel);
	d = opendir(p);
	if (!d)
		return;
	while ((e = readdir(d)) && nents < MAXENT) {
		struct stat st;
		char sub[256];

		if (!strcmp(e->d_name, ".") || !strcmp(e->d_name, ".."))
			continue;
		if (snprintf(sub, sizeof(sub), "%s%s%s", rel, *rel ? "/" : "",
			     e->d_name) >= (int)sizeof(sub))
			continue;
		snprintf(p, sizeof(p), "%s/%s", root, sub);
		if (lstat(p, &st))
			continue;
		strcpy(ents[nents], sub);
		types[nents++] = st.st_mode & S_IFMT;
		if (S_ISDIR(st.st_mode))
			scan(root, sub);
	}
	closedir(d);
}

/* An entry of type (0: any), or the root too when root is set. */
static inline const char *pick(int type, int root)
{
	int i, n = 0, idx[MAXENT + 1];

	for (i = 0; i < nents; i++)
		if (!type || types[i] == type)
			idx[n++] = i;
	if (root)
		idx[n++] = MAXENT;
	return n ? ents[idx[below(n)]] : NULL;
}

static inline void newname(char *out, size_t size)
{
	const char *dir = pick(S_IFDIR, 1);

	snprintf(out, size, "%s%sn%d", dir, *dir ? "/" : "", (int)below(40));
}

static inline void at(char *buf, const char *root, const char *rel)
{
	snprintf(buf, 512, "%s/%s", root, rel);
}

struct op {
	int kind;
	char a[256], b[256];
	uint64_t x, y, seed;
};

static inline int apply(const char *root, const struct op *o)
{
	char pa[512], pb[512], *buf;
	int fd, ret = 0;

	at(pa, root, o->a);
	at(pb, root, o->b);
	errno = 0;
	switch (o->kind) {
	case 0:
		fd = open(pa, O_WRONLY | O_CREAT | O_EXCL, 0644);
		if (fd < 0)
			return errno;
		buf = malloc(o->x + 1);
		for (uint64_t i = 0; i < o->x; i++)
			buf[i] = pattern(o->seed, i);
		if (write(fd, buf, o->x) != (ssize_t)o->x)
			ret = errno ? errno : EIO;
		free(buf);
		close(fd);
		return ret;
	case 1:
		fd = open(pa, O_WRONLY);
		if (fd < 0)
			return errno;
		buf = malloc(o->y + 1);
		for (uint64_t i = 0; i < o->y; i++)
			buf[i] = pattern(o->seed, i);
		if (pwrite(fd, buf, o->y, o->x) != (ssize_t)o->y)
			ret = errno ? errno : EIO;
		free(buf);
		close(fd);
		return ret;
	case 2:
		return truncate(pa, o->x) ? errno : 0;
	case 3:
		return mkdir(pa, 0755) ? errno : 0;
	case 4:
		return rmdir(pa) ? errno : 0;
	case 5:
		return unlink(pa) ? errno : 0;
	case 6:
		return renameat2(AT_FDCWD, pa, AT_FDCWD, pb, o->x) ? errno : 0;
	case 7:
		return link(pa, pb) ? errno : 0;
	case 8:
		return symlink(o->b, pa) ? errno : 0;
	case 9:
		buf = malloc(o->y + 1);
		for (uint64_t i = 0; i < o->y; i++)
			buf[i] = pattern(o->seed, i);
		snprintf(pb, sizeof(pb), "user.k%d", (int)o->x);
		ret = lsetxattr(pa, pb, buf, o->y, 0) ? errno : 0;
		free(buf);
		return ret;
	case 10:
		snprintf(pb, sizeof(pb), "user.k%d", (int)o->x);
		return lremovexattr(pa, pb) ? errno : 0;
	case 11:
		return chmod(pa, o->x) ? errno : 0;
	case 13: {
		loff_t in_off = o->x, out_off = o->y;
		size_t left = o->seed % 300000;
		ssize_t got = 0;
		int in = open(pa, O_RDONLY);

		if (in < 0)
			return errno;
		fd = open(pb, O_WRONLY);
		if (fd < 0) {
			ret = errno;
			close(in);
			return ret;
		}
		while (left && (got = copy_file_range(in, &in_off, fd, &out_off,
						      left, 0)) > 0)
			left -= got;
		if (got < 0)
			ret = errno;
		close(in);
		close(fd);
		return ret;
	}
	case 12:
		fd = open(pa, O_WRONLY | O_APPEND);
		if (fd < 0)
			return errno;
		buf = malloc(o->x + 1);
		for (uint64_t i = 0; i < o->x; i++)
			buf[i] = pattern(o->seed, i);
		if (write(fd, buf, o->x) != (ssize_t)o->x)
			ret = errno ? errno : EIO;
		free(buf);
		if (o->y)
			fsync(fd);
		close(fd);
		return ret;
	}
	return 0;
}

static inline void random_op(struct op *o)
{
	const char *p;
	int files = 0;

	memset(o, 0, sizeof(*o));
	for (int i = 0; i < nents; i++)
		files += types[i] == S_IFREG;
	o->seed = rnd();
	o->kind = below(14);
	if (nents >= MAXENT - 20 && (o->kind == 0 || o->kind == 3 ||
				     o->kind == 7 || o->kind == 8))
		o->kind = 5;
	switch (o->kind) {
	case 0:
	case 3:
	case 8:
		newname(o->a, sizeof(o->a));
		snprintf(o->b, sizeof(o->b), "t%d", (int)below(5));
		o->x = below(4) ? below(20000) : below(300000);
		break;
	case 1:
	case 2:
	case 12:
		p = pick(S_IFREG, 0);
		if (!p)
			return random_op(o);
		strcpy(o->a, p);
		o->x = below(4) ? below(70000) : below(400000);
		o->y = below(70000);
		if (o->kind == 12)
			o->x = below(9000);
		break;
	case 4:
	case 5:
		p = pick(o->kind == 4 ? S_IFDIR : 0, 0);
		if (!p)
			return random_op(o);
		strcpy(o->a, p);
		break;
	case 6:
		p = pick(0, 0);
		if (!p)
			return random_op(o);
		strcpy(o->a, p);
		p = below(3) ? NULL : pick(0, 0);
		if (p)
			strcpy(o->b, p);
		else
			newname(o->b, sizeof(o->b));
		switch (below(4)) {
		case 2:
			o->x = RENAME_NOREPLACE;
			break;
		case 3:
			o->x = RENAME_EXCHANGE;
			break;
		}
		break;
	case 7:
		p = files ? pick(S_IFREG, 0) : NULL;
		if (!p)
			return random_op(o);
		strcpy(o->a, p);
		newname(o->b, sizeof(o->b));
		break;
	case 9:
	case 10:
		strcpy(o->a, pick(0, 1));
		o->x = below(4);
		o->y = below(400);
		break;
	case 11:
		p = pick(S_IFREG, 0);
		if (!p)
			return random_op(o);
		strcpy(o->a, p);
		o->x = 0600 | (below(8) << 3);
		break;
	case 13:
		p = pick(S_IFREG, 0);
		if (!p)
			return random_op(o);
		strcpy(o->a, p);
		strcpy(o->b, pick(S_IFREG, 0));
		o->x = below(2) ? below(300000) : below(60) * 4096;
		o->y = below(2) ? below(300000) : below(60) * 4096;
		if (below(2))
			o->seed = o->seed / 4096 * 4096;
		break;
	}
}

static inline uint64_t fnv(uint64_t h, const void *p, size_t len)
{
	const uint8_t *b = p;

	while (len--)
		h = (h ^ *b++) * 0x100000001b3ULL;
	return h;
}

/* A digest of everything compared about a tree. */
static inline uint64_t digest(const char *root, const char *rel, uint64_t h)
{
	char p[1100], buf[4096];
	struct dirent **list;
	int n, i;

	snprintf(p, sizeof(p), "%s/%s", root, rel);
	n = scandir(p, &list, NULL, alphasort);
	if (n < 0)
		return fnv(h, "?", 1);
	for (i = 0; i < n; i++) {
		char sub[512], names[1024];
		struct stat st;
		ssize_t len;
		int fd;

		if (!strcmp(list[i]->d_name, ".") ||
		    !strcmp(list[i]->d_name, "..")) {
			free(list[i]);
			continue;
		}
		snprintf(sub, sizeof(sub), "%s/%s", rel, list[i]->d_name);
		snprintf(p, sizeof(p), "%s/%s", root, sub);
		free(list[i]);
		if (lstat(p, &st))
			continue;
		h = fnv(h, sub, strlen(sub) + 1);
		h = fnv(h, &st.st_mode, sizeof(st.st_mode));
		if (!S_ISDIR(st.st_mode)) {
			h = fnv(h, &st.st_nlink, sizeof(st.st_nlink));
			h = fnv(h, &st.st_size, sizeof(st.st_size));
		}
		len = llistxattr(p, names, sizeof(names));
		if (len > 0) {
			char *k = names;
			char *sorted[16];
			int nk = 0;

			while (k < names + len && nk < 16) {
				sorted[nk++] = k;
				k += strlen(k) + 1;
			}
			for (int a = 0; a < nk; a++)
				for (int b = a + 1; b < nk; b++)
					if (strcmp(sorted[a], sorted[b]) > 0) {
						char *t = sorted[a];

						sorted[a] = sorted[b];
						sorted[b] = t;
					}
			for (int a = 0; a < nk; a++) {
				ssize_t v = lgetxattr(p, sorted[a], buf,
						      sizeof(buf));

				h = fnv(h, sorted[a], strlen(sorted[a]));
				if (v > 0)
					h = fnv(h, buf, v);
			}
		}
		if (S_ISLNK(st.st_mode)) {
			len = readlink(p, buf, sizeof(buf));
			if (len > 0)
				h = fnv(h, buf, len);
		} else if (S_ISREG(st.st_mode)) {
			fd = open(p, O_RDONLY);
			while (fd >= 0 && (len = read(fd, buf, sizeof(buf))) > 0)
				h = fnv(h, buf, len);
			if (fd >= 0)
				close(fd);
		} else if (S_ISDIR(st.st_mode)) {
			h = digest(root, sub, h);
		}
	}
	free(list);
	return h;
}

static int seq;

static inline void compare(const char *a, const char *r, const char *when)
{
	uint64_t da = digest(a, "", 0xcbf29ce484222325ULL);
	uint64_t dr = digest(r, "", 0xcbf29ce484222325ULL);

	MUST(da == dr, "trees differ %s (op %d)", when, seq);
}

static inline int run_ops(const char *a, const char *r, int count, int verbose)
{
	struct op o;

	for (int i = 0; i < count; i++, seq++) {
		int ea, er;

		nents = 0;
		scan(r, "");
		random_op(&o);
		er = apply(r, &o);
		ea = apply(a, &o);
		if (ea != er) {
			printf("acfs: op %d kind %d %s %s %llu %llu: acfs %d, tmpfs %d\n",
			       seq, o.kind, o.a, o.b, (unsigned long long)o.x,
			       (unsigned long long)o.y, ea, er);
			MUST(0, "results differ");
		}
		if (verbose && !(i % 1000))
			printf("acfs: op %d\n", seq);
		if (!(i % 97))
			compare(a, r, "periodically");
	}
	return 0;
}

static struct {
	dev_t dev;
	ino_t ino;
	char path[512];
} links[MAXENT];
static int nlinks;

static inline void copy_xattrs(const char *from, const char *to)
{
	char names[1024], val[4096];
	ssize_t len = llistxattr(from, names, sizeof(names));

	for (char *k = names; len > 0 && k < names + len; k += strlen(k) + 1) {
		ssize_t v = lgetxattr(from, k, val, sizeof(val));

		MUST(v >= 0 && !lsetxattr(to, k, val, v, 0), "copy xattr");
	}
}

/* cp -a, keeping hard links and xattrs. */
static inline void copy_rec(const char *from, const char *to)
{
	char a[512], b[512], buf[4096];
	struct dirent *e;
	struct stat st;
	ssize_t n;
	int i, in, out;
	DIR *d;

	MUST(!lstat(from, &st), "lstat %s", from);
	if (S_ISDIR(st.st_mode)) {
		MUST(!mkdir(to, 0700), "mkdir %s", to);
		d = opendir(from);
		while ((e = readdir(d))) {
			if (!strcmp(e->d_name, ".") || !strcmp(e->d_name, ".."))
				continue;
			snprintf(a, sizeof(a), "%s/%s", from, e->d_name);
			snprintf(b, sizeof(b), "%s/%s", to, e->d_name);
			copy_rec(a, b);
		}
		closedir(d);
	} else if (S_ISLNK(st.st_mode)) {
		n = readlink(from, buf, sizeof(buf) - 1);
		MUST(n >= 0, "readlink");
		buf[n] = 0;
		MUST(!symlink(buf, to), "symlink %s", to);
		return;
	} else {
		for (i = 0; i < nlinks; i++)
			if (links[i].dev == st.st_dev && links[i].ino == st.st_ino) {
				MUST(!link(links[i].path, to), "link %s", to);
				return;
			}
		if (st.st_nlink > 1 && nlinks < MAXENT) {
			links[nlinks].dev = st.st_dev;
			links[nlinks].ino = st.st_ino;
			snprintf(links[nlinks++].path, 512, "%s", to);
		}
		in = open(from, O_RDONLY);
		out = open(to, O_WRONLY | O_CREAT | O_EXCL, 0600);
		MUST(in >= 0 && out >= 0, "copy %s", from);
		while ((n = read(in, buf, sizeof(buf))) > 0)
			MUST(write(out, buf, n) == n, "copy write");
		close(in);
		close(out);
	}
	copy_xattrs(from, to);
	MUST(!chmod(to, st.st_mode & 07777), "chmod %s", to);
}

static inline void copy_tree(const char *from, const char *to)
{
	nlinks = 0;
	copy_rec(from, to);
}

#endif

/* SPDX-License-Identifier: GPL-2.0 */
#ifndef ACFS_UTIL_H
#define ACFS_UTIL_H

#define _GNU_SOURCE
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <stdarg.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mount.h>
#include <sys/stat.h>
#include <sys/statfs.h>
#include <sys/wait.h>
#include <sys/xattr.h>
#include <unistd.h>
#include <linux/ac/acfs.h>
#include <linux/ac/acfs_tree.h>

#define DEV	"/dev/vda"

static int failures;

#define CHECK(cond, ...) do {						\
	if (!(cond)) {							\
		printf("acfs: FAIL %s:%d: %s: ", __FILE__, __LINE__, #cond); \
		printf(__VA_ARGS__);					\
		printf(" (errno %d)\n", errno);				\
		failures++;						\
	}								\
} while (0)

#define MUST(cond, ...) do {						\
	if (!(cond)) {							\
		CHECK(cond, __VA_ARGS__);				\
		exit(1);						\
	}								\
} while (0)

static inline void run(const char *cmd, ...)
{
	char *argv[16];
	va_list ap;
	int i = 0, status;
	pid_t pid;

	argv[i++] = (char *)cmd;
	va_start(ap, cmd);
	while ((argv[i] = va_arg(ap, char *)))
		i++;
	va_end(ap);
	pid = fork();
	if (!pid) {
		execv(cmd, argv);
		_exit(127);
	}
	MUST(waitpid(pid, &status, 0) == pid && WIFEXITED(status) &&
	     !WEXITSTATUS(status), "%s failed", cmd);
}

static inline void mkfs(void)
{
	run("/t/mkfs.acfs", DEV, NULL);
}

static inline void do_mount(const char *dir, const char *opts)
{
	mkdir(dir, 0755);
	MUST(!mount(DEV, dir, "acfs", 0, opts), "mount %s %s", dir,
	     opts ? opts : "");
}

static inline void do_umount(const char *dir)
{
	MUST(!umount(dir), "umount %s", dir);
}

static inline void drop_caches(void)
{
	int fd = open("/proc/sys/vm/drop_caches", O_WRONLY);

	sync();
	if (fd >= 0) {
		if (write(fd, "3", 1) != 1)
			perror("drop_caches");
		close(fd);
	}
}

static inline int put_file(const char *path, const void *data, size_t len)
{
	int fd = open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
	ssize_t n;

	if (fd < 0)
		return -1;
	n = write(fd, data, len);
	close(fd);
	return n == (ssize_t)len ? 0 : -1;
}

/* Whole file into a malloc'd buffer; *len its size. */
static inline char *get_file(const char *path, size_t *len)
{
	struct stat st;
	char *buf;
	int fd = open(path, O_RDONLY);
	size_t got = 0;

	if (fd < 0 || fstat(fd, &st))
		return NULL;
	buf = malloc(st.st_size + 1);
	while (got < (size_t)st.st_size) {
		ssize_t n = read(fd, buf + got, st.st_size - got);

		if (n <= 0)
			break;
		got += n;
	}
	close(fd);
	buf[got] = 0;
	*len = got;
	return buf;
}

static inline int file_is(const char *path, const char *want)
{
	size_t len;
	char *got = get_file(path, &len);
	int ok = got && len == strlen(want) && !memcmp(got, want, len);

	free(got);
	return ok;
}

static inline uint8_t pattern(uint64_t seed, uint64_t off)
{
	uint64_t x = (seed + off / 8) * 0x9e3779b97f4a7c15ULL;

	x ^= x >> 29;
	return (uint8_t)(x >> (8 * (off % 8)));
}

static inline int branch_create(const char *dir, uint64_t src,
				const char *name, uint64_t flags,
				struct acfs_branch_info *info)
{
	struct acfs_branch_create c = { .size = sizeof(c), .src = src,
					.flags = flags };
	int fd = open(dir, O_RDONLY | O_DIRECTORY), ret;

	if (fd < 0)
		return -1;
	snprintf(c.name, sizeof(c.name), "%s", name);
	ret = ioctl(fd, ACFS_IOC_BRANCH_CREATE, &c);
	close(fd);
	if (!ret && info)
		*info = c.info;
	return ret;
}

static inline int branch_delete(const char *dir, uint64_t id)
{
	struct acfs_branch_delete d = { .size = sizeof(d), .id = id };
	int fd = open(dir, O_RDONLY | O_DIRECTORY), ret;

	if (fd < 0)
		return -1;
	ret = ioctl(fd, ACFS_IOC_BRANCH_DELETE, &d);
	close(fd);
	return ret;
}

static inline int branch_get(const char *dir, uint64_t id, const char *name,
			     struct acfs_branch_info *info)
{
	struct acfs_branch_get g = { .size = sizeof(g), .id = id };
	int fd = open(dir, O_RDONLY | O_DIRECTORY), ret;

	if (fd < 0)
		return -1;
	if (name)
		snprintf(g.name, sizeof(g.name), "%s", name);
	ret = ioctl(fd, ACFS_IOC_BRANCH_GET, &g);
	close(fd);
	if (!ret)
		*info = g.info;
	return ret;
}

static inline uint64_t free_blocks(const char *dir)
{
	struct statfs s;

	sync();
	if (statfs(dir, &s))
		return 0;
	return s.f_bfree;
}

#endif

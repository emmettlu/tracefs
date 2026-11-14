# SPDX-License-Identifier: GPL-2.0
# Static, so that vm.sh can run them in a minimal initramfs.
CFLAGS += -static -O2 -Wall -pthread $(KHDR_INCLUDES)
LDLIBS += -pthread

TEST_GEN_PROGS := acfs_basic acfs_stress acfs_concurrent acfs_crash
TEST_GEN_PROGS_EXTENDED := mkfs.acfs fsck.acfs
TEST_PROGS_EXTENDED := vm.sh

include ../../lib.mk

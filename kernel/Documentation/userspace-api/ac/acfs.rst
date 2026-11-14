.. SPDX-License-Identifier: GPL-2.0

====================================
acfs: the Agent Computer File System
====================================

acfs is a copy-on-write file system with branches. A branch is a tree of
files; making one from another is O(1), and the two share every block
until either side changes it. AgentOS gives each agent its own branch of a
workspace, takes read-only snapshots as changesets, compares them, and
merges branches back. The merge itself is userspace's business; acfs
provides the branches, the snapshots, the comparison and cheap copies.

The code is in ``fs/acfs/``; ``include/uapi/linux/ac/acfs.h`` is the ioctl
ABI and ``include/uapi/linux/ac/acfs_tree.h`` the on-disk format.
``tools/testing/selftests/ac/acfs/`` has ``mkfs.acfs``, ``fsck.acfs`` and
the tests.

Using it
========

::

  mkfs.acfs /dev/sdX
  mount -t acfs /dev/sdX /work                    # the main branch
  mount -t acfs -o branch=agent1 /dev/sdX /a1     # another branch
  mount -t acfs -o branchid=7 /dev/sdX /snap      # by id

A device has one super_block whatever is mounted; every mount of it shows
one branch's root. Files of a branch other than main report a st_dev of
their own branch. The same file in two branches has the same inode number
(its identity as of the branch point); inode numbers come from one counter
for the whole file system, so files made in different branches never
share one.

A read-only branch (a snapshot) refuses every change with ``EROFS``.

Branch ioctls
=============

They work on any fd of the file system. Structures start with their size
and only grow at their end. "Needs to manage" means ``CAP_SYS_ADMIN`` or
owning the root directory of the mount the fd is on, so a user can keep
snapshots of a workspace file system they own.

``ACFS_IOC_BRANCH_CREATE`` (``struct acfs_branch_create``)
  Make branch ``name`` from branch ``src`` (0: the fd's branch) as it is
  now: the file system is written back first, then the new branch's root
  is the source's root with one more reference, and the transaction
  commits. ``flags`` may be ``ACFS_BRANCH_RDONLY``. Names are unique, 1 to
  63 bytes, without ``,``, ``/`` or newlines. Open-but-unlinked files of
  the source do not carry over. Needs to manage.

``ACFS_IOC_BRANCH_DELETE`` (``struct acfs_branch_delete``)
  The branch disappears from lookups at once; its blocks are freed when
  nothing uses it any more: unused cached dentries and inodes are dropped
  right away, mounts and open files keep it until they go. Main cannot be
  deleted (``EBUSY``). A crash before the blocks are freed is finished at
  the next mount. Needs to manage.

``ACFS_IOC_BRANCH_GET`` (``struct acfs_branch_get``)
  One branch by id, else by name, else the fd's.

``ACFS_IOC_BRANCH_LIST`` (``struct acfs_branch_list``)
  Branches with ids from ``start`` on, ``count`` at a time.

``ACFS_IOC_BRANCH_OPEN`` (``struct acfs_branch_open``)
  A new fd, read-only, of the root directory of one branch (by id, else by
  name) on the fd's mount: its files are read with ``openat()`` and
  friends without mounting it. ``flags`` may be ``O_CLOEXEC``. ``..`` from
  that directory stays in it. Needs to manage.

``ACFS_IOC_DIFF`` (``struct acfs_diff``)
  The items that differ between branches ``a`` and ``b`` (usually two
  snapshots), in key order, resumable from ``pos``. The two trees are
  walked together and every subtree they share is skipped, so the cost
  follows the size of the difference, not of the trees. Entries are keys
  of ``acfs_tree.h``: an inode changed when any item with its number as
  objectid is listed; ``ACFS_DIRENT_KEY`` entries give the hashes of the
  names that changed in a directory, ``ACFS_EXTENT_KEY`` entries the
  blocks of a file. Needs to manage.

Copies
======

``copy_file_range()``, ``FICLONE``, ``FICLONERANGE`` and
``FIDEDUPERANGE`` share blocks instead of copying them, for block-aligned
ranges (and a source range ending at the source's end of file).
``copy_file_range()`` does this between branches too; ``FICLONE`` wants
both files on one mount. Unaligned ranges are copied by the VFS.

On disk
=======

Blocks are 4 KiB. Block 0 and 1 are the two superblock slots, written in
turn (slot ``gen % 2``); the valid one with the highest ``gen`` is current.
Then comes the refcount table: one ``__le32`` per block, in pages of
``ACFS_RC_PER_PAGE``, each page in two slots. The valid slot with the
highest ``gen`` not above the superblock's is current; a transaction writes
the other one. Everything after that is tree nodes and file data. Each
block of metadata carries a crc32c from its byte 4 on.

Trees are B+ trees of variable-size items under ``(objectid, type,
offset)`` keys, copied on write with lazy reference counts: a block's
count is the number of tree nodes, extent items or branch items that point
at it, however many trees share those. Making a branch adds one reference
to a root; changing a shared node copies it and gives its children one
more reference each. A node is changed in place only when the running
transaction wrote it and nothing else refers to it.

The root tree has one ``acfs_branch_item`` per branch. A branch's tree
holds its files:

=========================  ==========================================
``(ino, INODE, 0)``        ``acfs_inode_item``
``(ino, INLINE, 0)``       a symlink's target
``(ino, XATTR, hash)``     extended attributes whose name has the hash
``(dir, DIRENT, hash)``    entries whose name has the hash (xxh64)
``(dir, DIRINDEX, n)``     the same entry, at readdir position n
``(ino, EXTENT, last)``    file blocks [last - len + 1, last]
``(ORPHAN, ORPHAN, ino)``  unlinked while open: deleted at next mount
=========================  ==========================================

A directory's size is the sum of its names' lengths.

Transactions and crashes
========================

Nothing the last commit refers to is overwritten before the next commit
lands: nodes and file data go to new blocks, a block freed by the running
transaction stays in use until the commit, and refcount pages alternate
slots. So any block but the superblock may reach the disk early. A commit
waits for the transaction's data writes, writes back the block device and
writes the superblock with a preflush and FUA. Commits happen every 5
seconds, on ``sync()``, and on ``fsync()`` (which commits everything; there
is no log tree yet). After a crash the file system is as of the last
commit; orphans and deleted branches are finished at mount.

An error in the middle of a change (``EIO``, ``ENOMEM``, a damaged node)
aborts the transaction: nothing more is committed, the disk stays at the
last commit, and the file system has to be mounted again. Changes that
need new blocks fail with ``ENOSPC`` once free space is below a reserve
(1/64 of the device, at most 1 GiB), which keeps deletions and commits
going.

Limits
======

- Page size must be 4 KiB; one folio is one block.
- One lock for all trees: metadata changes are serialized.
- Mount reads the whole refcount table (0.2% of the device).
- No direct I/O, data checksums, compression, quotas, ACLs or log tree
  yet; ``security.*`` xattrs are kept but new files get no LSM label.
- Leaves are not rebalanced after deletions, only removed when empty.

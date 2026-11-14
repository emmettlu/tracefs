#!/bin/sh
# Build an Arch Linux ARM (aarch64) disk image bootable by Lima's vz driver:
# GPT, ESP (vfat) mounted at /boot with GRUB (removable path), btrfs root,
# cloud-init for Lima's cidata. Run as root on any aarch64 Linux (tested: Alpine).
# Usage: build-alarm-image.sh OUTDIR  -> OUTDIR/arch-aarch64.qcow2
set -eux
OUT=${1:?outdir}
W=$OUT/work
R=$W/root
mkdir -p "$W" "$R"
cd "$W"

command -v apk >/dev/null && apk add --no-cache btrfs-progs dosfstools sgdisk qemu-img libarchive-tools util-linux curl
[ -s rootfs.tgz ] || curl -fL -o rootfs.tgz.part http://os.archlinuxarm.org/os/ArchLinuxARM-aarch64-latest.tar.gz && mv rootfs.tgz.part rootfs.tgz 2>/dev/null || true
[ -s rootfs.tgz ]

rm -f disk.raw
truncate -s 12G disk.raw
sgdisk -Z -n1:0:+1G -t1:ef00 -c1:ESP -n2:0:0 -t2:8305 -c2:root disk.raw
LOOP=$(losetup -fP --show disk.raw)
cleanup() {
	set +e
	for m in run dev/pts dev sys proc boot; do umount "$R/$m" 2>/dev/null; done
	umount "$R" 2>/dev/null
	losetup -d "$LOOP"
}
trap cleanup EXIT
mkfs.vfat -F32 -n ESP "${LOOP}p1"
mkfs.btrfs -f -L archroot "${LOOP}p2"
mount -o compress=zstd:3 "${LOOP}p2" "$R"
bsdtar -xpf rootfs.tgz -C "$R"

# move tarball /boot onto the ESP
mkdir -p "$W/esp"
mount "${LOOP}p1" "$W/esp"
cp -r "$R/boot/." "$W/esp/" 2>/dev/null || true
umount "$W/esp"
rm -rf "$R/boot" && mkdir "$R/boot"
mount "${LOOP}p1" "$R/boot"

for m in proc sys dev dev/pts run; do mkdir -p "$R/$m"; done
mount -t proc proc "$R/proc"
mount --rbind /sys "$R/sys"
mount --rbind /dev "$R/dev"
mount -t tmpfs tmpfs "$R/run"
mv "$R/etc/resolv.conf" "$R/etc/resolv.conf.orig" 2>/dev/null || true
cp -L /etc/resolv.conf "$R/etc/resolv.conf"

ROOTUUID=$(blkid -s UUID -o value "${LOOP}p2")
ESPUUID=$(blkid -s UUID -o value "${LOOP}p1")
cat >"$R/etc/fstab" <<FSTAB
UUID=$ROOTUUID / btrfs rw,noatime,compress=zstd:3 0 0
UUID=$ESPUUID /boot vfat rw,noatime,umask=0077 0 2
FSTAB

cat >"$R/tmp/inside.sh" <<'IN'
set -eux
pacman-key --init
pacman-key --populate archlinuxarm
# the geo-redirecting mirror.archlinuxarm.org stalls on large files
cat >/etc/pacman.d/mirrorlist <<'ML'
Server = http://mirrors.tuna.tsinghua.edu.cn/archlinuxarm/$arch/$repo
Server = http://mirrors.ustc.edu.cn/archlinuxarm/$arch/$repo
Server = https://mirrors.aliyun.com/archlinuxarm/$arch/$repo
Server = http://fl.us.mirror.archlinuxarm.org/$arch/$repo
ML
sed -i 's/^#\?ParallelDownloads.*/ParallelDownloads = 4/' /etc/pacman.conf
retry() { for i in 1 2 3; do "$@" && return; sleep 5; done; return 1; }
# no autodetect: the build host is not the target VM
sed -i 's/^MODULES=.*/MODULES=(virtio_pci virtio_blk virtio_net virtio_console btrfs vfat)/; s/^HOOKS=.*/HOOKS=(base udev modconf block filesystems keyboard fsck)/' /etc/mkinitcpio.conf
retry pacman -Syu --noconfirm
retry pacman -S --noconfirm --needed linux-aarch64 grub efibootmgr btrfs-progs dosfstools gptfdisk \
	cloud-init cloud-guest-utils sudo openssh
KVER=$(ls /usr/lib/modules | grep -v extramodules | sort -V | tail -1)
mkinitcpio -k "$KVER" -g /boot/initramfs-linux.img
grub-install --target=arm64-efi --efi-directory=/boot --boot-directory=/boot --removable --no-nvram
userdel -r alarm 2>/dev/null || true
passwd -l root
for u in sshd systemd-networkd systemd-resolved cloud-init-main cloud-init-local cloud-init-network cloud-init cloud-config cloud-final; do
	[ -e /usr/lib/systemd/system/$u.service ] && systemctl enable $u.service
done
rm -rf /var/cache/pacman/pkg/* /var/lib/cloud
: >/etc/machine-id
IN
chroot "$R" /bin/bash /tmp/inside.sh
rm -f "$R/tmp/inside.sh"

cat >"$R/boot/grub/grub.cfg" <<CFG
set timeout=1
menuentry "Arch Linux ARM" {
	search --no-floppy --fs-uuid --set=root $ESPUUID
	linux /Image root=UUID=$ROOTUUID rw rootflags=compress=zstd:3 console=tty0 console=hvc0
	initrd /initramfs-linux.img
}
CFG
ls -la "$R/boot" "$R/boot/EFI/BOOT"

rm -f "$R/etc/resolv.conf"
[ -e "$R/etc/resolv.conf.orig" ] || [ -L "$R/etc/resolv.conf.orig" ] && mv "$R/etc/resolv.conf.orig" "$R/etc/resolv.conf"
cleanup
trap - EXIT
qemu-img convert -O qcow2 disk.raw "$OUT/arch-aarch64.qcow2"
ls -la "$OUT/arch-aarch64.qcow2"

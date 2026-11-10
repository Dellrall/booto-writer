#!/usr/bin/env bash
# End-to-end check on a Linux CI runner (needs sudo): makes Booto USBs from the newest Booto release
# (--channel nightly) on a 4 GiB scsi_debug disk the kernel reports as removable, without and with an
# encrypted report partition, checks the layout, raw-writes the ISO, and checks the system-disk guard.
# Usage: scripts/test-linux.sh <booto-writer binary>
set -euo pipefail
bin="$(realpath "${1:?usage: $0 <booto-writer binary>}")"
pass='ci passphrase'
bw() { sudo --preserve-env=GITHUB_TOKEN "$bin" "$@"; }
fail() { echo "FAIL: $*" >&2; sudo lsblk -o PATH,SIZE,TYPE,PARTTYPE,PARTLABEL,FSTYPE,LABEL "$dev" >&2 || true; exit 1; }
mnt="$(mktemp -d)"
trap 'sudo umount -q "$mnt" 2>/dev/null || true; sudo cryptsetup close bw_test 2>/dev/null || true' EXIT

sudo apt-get install -y -qq gdisk dosfstools e2fsprogs parted cryptsetup > /dev/null
if ! sudo modprobe scsi_debug dev_size_mb=4096 removable=1 2> /dev/null; then
  sudo apt-get install -y -qq "linux-modules-extra-$(uname -r)" > /dev/null
  sudo modprobe scsi_debug dev_size_mb=4096 removable=1
fi
sudo udevadm settle
dev="$(lsblk -dnpo PATH,MODEL | awk '$2 == "scsi_debug" { print $1; exit }')"
[ -n "$dev" ] || fail "no scsi_debug disk appeared"
echo "Test disk: $dev"
"$bin" list | grep -q "^$dev " || fail "list does not show $dev as removable"

# lsblk field of one partition, read fresh from udev.
field() { sudo udevadm settle; lsblk -dnro "$2" "${dev}$1"; }
check_live() {
  [ "$(field 1 PARTTYPE)" = c12a7328-f81f-11d2-ba4b-00a0c93ec93b ] || fail "partition 1 is not an EFI system partition"
  [ "$(field 1 PARTLABEL)" = BOOTO_LIVE ] && [ "$(field 1 LABEL)" = BOOTO_LIVE ] && [ "$(field 1 FSTYPE)" = vfat ] \
    || fail "partition 1 is not a vfat BOOTO_LIVE"
  [ "$(field 1 SIZE)" = 3G ] || fail "partition 1 is $(field 1 SIZE), not 3G"
  [ "$(field 2 PARTLABEL)" = BOOTO_DATA ] || fail "partition 2 is not BOOTO_DATA"
  sudo mount -o ro "${dev}1" "$mnt"
  for f in EFI/BOOT/BOOTX64.EFI boot/grub/grub.cfg live/vmlinuz live/initrd.img; do
    sudo test -s "$mnt/$f" || fail "$f is missing on BOOTO_LIVE"
  done
  sudo umount "$mnt"
}
check_data() { # <block device holding the report filesystem>
  [ "$(sudo blkid -o value -s LABEL "$1")" = persistence ] || fail "report filesystem is not labelled persistence"
  sudo mount "$1" "$mnt"
  [ "$(sudo cat "$mnt/persistence.conf")" = "/ union" ] || fail "persistence.conf is wrong"
  for d in cases rules logs; do
    [ "$(sudo stat -c %a "$mnt/$d")" = 700 ] || fail "$d is not mode 700"
  done
  sudo umount "$mnt"
}

echo "== booto --no-encrypt"
bw booto "$dev" --channel nightly --no-encrypt --yes
check_live
[ "$(field 2 PARTTYPE)" = 0fc63daf-8483-4772-8e79-3d69d8477de4 ] || fail "partition 2 is not type 8300"
check_data "${dev}2"

echo "== booto with an encrypted 256M report partition (passphrase on stdin)"
printf '%s\n' "$pass" | bw booto "$dev" --channel nightly --data-size 256M --yes
check_live
[ "$(field 2 PARTTYPE)" = ca7d7ccb-63ed-4c53-861c-1742536059cc ] || fail "partition 2 is not type 8309"
[ "$(field 2 SIZE)" = 256M ] || fail "partition 2 is $(field 2 SIZE), not 256M"
# Booto types the passphrase without a newline; so does this.
printf '%s' "$pass" | sudo cryptsetup open --key-file=- "${dev}2" bw_test || fail "the passphrase does not open the report partition"
check_data /dev/mapper/bw_test
sudo cryptsetup close bw_test

echo "== write (raw) and read-back verify"
iso="$(sudo sh -c 'ls "$HOME"/.cache/booto-writer/booto-*.iso' | head -n1)"
[ -n "$iso" ] || fail "no cached Booto ISO"
bw write "$iso" "$dev" --yes

echo "== the system disk is refused, even with --force-nonremovable"
root="$(lsblk -nsrpo PATH,TYPE "$(findmnt -nvro SOURCE /)" | awk '$2 == "disk" { print $1; exit }')"
# The image does not exist: if the guard ever failed, the write would stop before touching the disk.
if out="$(bw write /nonexistent.iso "$root" --force-nonremovable --yes 2>&1)"; then fail "writing $root was allowed"; fi
grep -q "in use by the running system" <<< "$out" || fail "unexpected refusal for $root: $out"

echo "Linux end-to-end OK on $dev"

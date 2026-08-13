#!/usr/bin/env python3
"""Guest conformance proof for the selected-root OverlayFS profile."""

import errno
import json
import os
import shutil
import stat
import subprocess
import tempfile


root = tempfile.mkdtemp(prefix="conary-overlay-profile-")
lower = os.path.join(root, "lower")
upper = os.path.join(root, "upper")
work = os.path.join(root, "work")
merged = os.path.join(root, "merged")
for path in (lower, upper, work, merged):
    os.mkdir(path)

with open(os.path.join(lower, "hardlink-a"), "wb") as stream:
    stream.write(b"hardlink-content")
os.link(os.path.join(lower, "hardlink-a"), os.path.join(lower, "hardlink-b"))
with open(os.path.join(lower, "remove-me"), "wb") as stream:
    stream.write(b"remove")
os.mkdir(os.path.join(lower, "replace-me"))
with open(os.path.join(lower, "replace-me", "lower-child"), "wb") as stream:
    stream.write(b"lower")
os.mkdir(os.path.join(lower, "rename-me"))
with open(os.path.join(lower, "rename-me", "lower-child"), "wb") as stream:
    stream.write(b"lower")

options = ",".join(
    [
        f"lowerdir={lower}",
        f"upperdir={upper}",
        f"workdir={work}",
        "index=on",
        "redirect_dir=nofollow",
        "metacopy=off",
        "xino=off",
        "nfs_export=off",
        "verity=off",
    ]
)
mounted = False
try:
    subprocess.run(
        ["mount", "-t", "overlay", "overlay", "-o", options, merged], check=True
    )
    mounted = True

    changed = os.path.join(merged, "hardlink-a")
    alias = os.path.join(merged, "hardlink-b")
    os.chmod(changed, 0o600)
    changed_stat = os.lstat(changed)
    alias_stat = os.lstat(alias)
    assert (changed_stat.st_dev, changed_stat.st_ino) == (
        alias_stat.st_dev,
        alias_stat.st_ino,
    )
    assert stat.S_IMODE(changed_stat.st_mode) == 0o600
    assert stat.S_IMODE(alias_stat.st_mode) == 0o600
    with open(os.path.join(upper, "hardlink-a"), "rb") as stream:
        assert stream.read() == b"hardlink-content"
    assert os.getxattr(os.path.join(upper, "hardlink-a"), "trusted.overlay.origin")
    try:
        os.getxattr(os.path.join(upper, "hardlink-a"), "trusted.overlay.metacopy")
    except OSError as error:
        assert error.errno in (errno.ENODATA, errno.EOPNOTSUPP)
    else:
        raise AssertionError("metacopy marker emitted despite metacopy=off")

    os.unlink(os.path.join(merged, "remove-me"))
    whiteout = os.path.join(upper, "remove-me")
    whiteout_stat = os.lstat(whiteout)
    if stat.S_ISCHR(whiteout_stat.st_mode) and whiteout_stat.st_rdev == 0:
        whiteout_encoding = "character-device-zero-zero"
    else:
        assert stat.S_ISREG(whiteout_stat.st_mode) and whiteout_stat.st_size == 0
        os.getxattr(whiteout, "trusted.overlay.whiteout")
        whiteout_encoding = "zero-size-regular-xattr"

    shutil.rmtree(os.path.join(merged, "replace-me"))
    os.mkdir(os.path.join(merged, "replace-me"))
    assert (
        os.getxattr(os.path.join(upper, "replace-me"), "trusted.overlay.opaque")
        == b"y"
    )

    try:
        os.rename(os.path.join(merged, "rename-me"), os.path.join(merged, "renamed"))
    except OSError as error:
        assert error.errno == errno.EXDEV
    else:
        raise AssertionError("lower directory renamed despite redirect_dir=nofollow")

    os.sync()
    print(
        json.dumps(
            {
                "kernel": os.uname().release,
                "whiteout_encoding": whiteout_encoding,
                "opaque_directory": "logical-y",
                "hardlink_copy_up": "preserved",
                "lower_directory_rename": "cross-device",
                "metadata_copy_up": "complete-data",
            },
            sort_keys=True,
        )
    )
finally:
    if mounted:
        subprocess.run(["umount", merged], check=True)
    shutil.rmtree(root)

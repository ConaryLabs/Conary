#!/usr/bin/env python3
# apps/conary/tests/fixtures/selected-root-current-generation/prepare.py

"""Install a minimal exact generation artifact for selected-root mount proof."""

import base64
import pathlib
import sys
import zlib


FILES = {
    "generations/1/.conary-artifact.json": "eNp1ktFu2zAMRd/7FYGfl1SyKIrazxgUTa0umniw1KFF0X+f7LSIO3SAYfjykuKxyLe7w6H7o0uZ5kv38+B+rPqXXnTheg3ZLcSLPExVpT4v2oLdC+GA0G2e8LJMugzCvzlNT1OdtLSct+Z9Ody2wPtWcdbKI1deTzrJfOHl9dianh5LS9wydJnzeki3zHM9XdXNGMoD9x5X34qQEdNbVpucz2Mw0TqlnjwkI8ASDSBjCJADoBhJKaLTUT1wBt/988fD2nA482XKWura4eYdN5gb5P+qdngYrALZHFwSsYGcpDjmJD5BjkmNN2IVM7Q4Uu/6fn05jwlNQFK6Njo/V05POpTKVb/AfTjHzdmhfV+xA3NWXKas0WX24ExMaHsUyIY4Sy+RQkgkFKHBZBc0ETjHDKJJncufoy9D4rKtxOl03545PbYtKTd7T9v08VPvYPdpO8QUbfCiEcVCMD7FjJ5sjH0Aw+1yLDKkGN3Io4eW02ZuKeZ2hywGx4/tTOtkuBSt2z6t8niV99+g7LJ3JNQmEYPACJIjqcFA2XBj0uxH9K59gtAobb88crZoesemTTDESBDG7u79L+wOBLg=",
    "generations/1/.conary-gen.json": "eNpdUctuwyAQvOcrLJ8biYexId/RUy/WArupG9tEQNqkVf+92E4fqoQE7GNmNPOxq6r6iDNGyEOY60PFH5YShThBLt/ahekcElKq1wbGQKlPwzuWZsNMu1YdpD7YF3Q59REJI84OfZlgG1p6xTjkW48z2HFtEIwJ/yDeB/xwxLTyMt3ajglQ3FjV+RZJAUJL2koSFkUjlXKek9Wy3JqRbVB6qbUVjVegNrkQ80Dgcj/BPFCB7tMzCNUuDKCNYCBBmYJipOWm49g0pnXIpbK+9aIjMt6rThvJGQfSjUcOCw1JvjEkdJdVuoMz2GFcnlfIOfYuXOb844GLCBl9v7kqmGj3TO+5fGTssJ6nDe8M7gRH/Ld9wjjjuLiUtphqGq75EvEu4jJNEG9LPeFYYkC/jyHkqmgrWeT9b8TVGN4wVt/ru88vqsaeyA==",
    "generations/1/boot-assets/EFI/BOOT/BOOTX64.EFI": "eNpLy6woKS1K1U1NywQAG08EaQ==",
    "generations/1/boot-assets/initramfs.img": "eNpLy6woKS1K1c3MyywpSsxNKwYAPyUHAg==",
    "generations/1/boot-assets/manifest.json": "eNptkE9PG0EMxe98imivhWCP1/MnRyQq9cSFQ2+Rx+MJqyYbabNEEYjv3t1QlYC4WPJ79m/e+PVqsWiONhy6fd+sFng99xvrbZDxUpJBn7rRdHwebBKbU/Rr3zZn748NvW3XH5Smdqfz4IU9y8fdtuufXz5tHZ7EsZ9dFYNKbU5cuMZC1TsiY5LkOHpsDQNX4uACK6fsEJyqUUtcElnFd2zXd+Mgu3qYkf+bZbfbfPEvXnbBIRVKLkCaUmBFV1nYKAIqUsihxhgkRgVuQZXZkfdZfQRh8vJOttqt834/bvdSbJi59z9/3d49PDyey2/fLifhu9mLKNkyZiDUhJlzgba0KVfHRRiNFAJJFJ3yTLdSMEOY7qRkKtkQ4V8UHUxGK2sZz98D528g3iA9Iq+YVi4uY8IQOTr+AbACaK7e/gLwVZJB",
    "generations/1/boot-assets/vmlinuz": "eNpLy6woKS1K1c1OLcpLzQEAK00Ftg==",
    "generations/1/cas-manifest.json": "eNqr5lJQUCpLLSrOzM9TslIw1AHx01PzUosSS5CFEouSMzJLUpNLSotSgYJKFRZm8WYmSmC5/KQsoEQxUDg6lqsWAMfeF1M=",
    "generations/1/generation-root.json": "eNqljrEOwjAMRPd+RZW5A10o4lcQQ5VYlYUSV46DQFX/nRhKCSobW+7e3TlTVdfmChyRgjnWbaOaiSSLKb+zipTYwqqzc8HgCp0duY+aMA4ZrBDfzcLm5l3y5DTS7g9dt5opAv9eCskDozXNB6Ee3W2GB6Y0/jviBT18j0SwFFzUeNEPfaCCbIZuvQgrmuaqICa9Di9qWL/x5AaCMILWTudqfgBg9Vza",
    "generations/1/mutable-state.json": "eNqr5lJQUCpLLSrOzM9TslIw1AHxU/NKijJTi4H86FiuWgCveAno",
    "generations/1/root.erofs": "eNqblVRxgZEBApgYRsEoGAUjCTx6+PUBiGYDYh4GFQZGLGoYaWg/KxC/dWRgkEYSU8GhFlv5BFMrAZSFsSWBbD09vdHIHQWjYBSMglEwCkbBKBgFowANAABj5giT",
}


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit("usage: prepare.py RUNTIME_ROOT")
    runtime_root = pathlib.Path(sys.argv[1])
    runtime_root.mkdir(parents=True, exist_ok=True)
    (runtime_root / "objects").mkdir(exist_ok=True)
    for relative, encoded in FILES.items():
        destination = runtime_root / relative
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(zlib.decompress(base64.b64decode(encoded)))
    current = runtime_root / "current"
    if current.exists() or current.is_symlink():
        current.unlink()
    current.symlink_to("generations/1")


if __name__ == "__main__":
    main()

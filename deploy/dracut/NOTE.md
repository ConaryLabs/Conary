# deploy/dracut vs packaging/dracut

These are **not** duplicates.

- `deploy/dracut/` -- Simplified boot hook for deployed systems. Calls
  `conary system generation recover` which handles the full 4-step fallback
  internally. Used in production deployments where the conary binary is
  available at boot.

- `packaging/dracut/90conary/` -- Standalone dracut module for packaged
  installations. Handles EROFS + composefs mounting directly in shell
  (kernel cmdline parsing, composefs mount, /etc overlay) without requiring
  the conary binary at initramfs time.

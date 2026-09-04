# deploy/dracut vs packaging/dracut

These are separate implementations of the same boot responsibility and are
scheduled for consolidation in [#863](https://github.com/FieldmouseWorks/Conary/issues/863).
The verification policy owner is conary-core's `generation::verity_policy::VerityPolicy`.

- `deploy/dracut/` -- Simplified boot hook for deployed systems. Calls
  `conary system generation recover` which handles the full 4-step fallback
  internally. The conary binary is installed in this initramfs and recovery
  consumes the Rust policy directly, before scanning or mounting generations.

- `packaging/dracut/90conary/` -- Standalone dracut module for packaged
  installations. Handles EROFS + composefs mounting directly in shell
  (kernel cmdline parsing, composefs mount, /etc overlay) without requiring
  the conary binary at initramfs time. Its shared shell adapter is tested for
  conformance to the Rust policy.

- `bootstrap/system_config.rs` in conary-core generates a third `/init`, also
  without a conary binary in its declared inputs. It embeds the same shell
  adapter. Neither binary-free image can delegate to conary before switch_root.

---
last_updated: 2026-07-24
revision: 1
summary: Define authoritative RPM, Debian, and Arch lifecycle parsing and adapter requirements
---

# Foreign Package Lifecycle Contracts

This specification defines how Conary turns RPM, Debian, and Arch package
lifecycle behavior into native CCS authority. It separates finite
package-manager contracts from the arbitrary shell programs that can appear
inside those contracts.

## Authority Rule

The authority pipeline is:

1. The native package parser identifies the package manager, lifecycle slot,
   interpreter, body, and package metadata.
2. A formal shell grammar parses shell bodies into command nodes. Syntax
   errors become typed parse diagnostics; they do not produce guessed command
   strings.
3. Each command node records exact command and argument provenance plus
   execution context.
4. An adapter matches an exact documented helper grammar.
5. The adapter validates the declared effect against package payload or
   persisted state and names the Conary-native replacement model.
6. Only a complete native model can establish compatibility or publication
   authority.

Heuristics, regex detections, normalized strings, and corpus frequency may
redact evidence or prioritize adapter work. They never establish semantic
equivalence, public serving, host mutation, or security authority.

The shell grammar is implemented with
[`tree-sitter-bash`](https://github.com/tree-sitter/tree-sitter-bash).
`crates/conary-core/src/ccs/convert/command_evidence.rs` owns AST extraction;
`adapters.rs` owns the registry and authority gate;
`adapters/builtin.rs` owns cross-distribution helper implementations; and the
distribution-specific modules own their exact helper grammars.

## RPM Surface

Authoritative upstream references:

- [RPM spec scriptlet and trigger syntax](https://rpm.org/docs/4.20.x/manual/spec.html)
- [RPM trigger semantics](https://rpm.org/docs/latest/manual/triggers.html)
- [RPM scriptlet execution source](https://github.com/rpm-software-management/rpm/blob/master/lib/rpmscript.cc)
- [RPM package-state-machine source](https://github.com/rpm-software-management/rpm/blob/master/lib/psm.cc)

The package-level surface is finite: ordinary install/uninstall scriptlets,
transaction scriptlets, file and package triggers, interpreter metadata, and
their RPM-defined argument conventions. Bodies using a shell interpreter enter
the formal shell pipeline. Non-shell interpreters remain typed native bodies
and require interpreter-specific adapters; Conary must not tokenize them as
shell.

RPM macros that expand into helpers are accepted only through the expanded
package script body or a parser-owned typed macro contract. Macro names or
substring matches are not authority.

## Debian Surface

Authoritative upstream references:

- [Debian Policy maintainer-script contract](https://www.debian.org/doc/debian-policy/ch-maintainerscripts.html)
- [dpkg source](https://sources.debian.org/src/dpkg/)
- [`dpkg-maintscript-helper` grammar and behavior](https://manpages.debian.org/testing/dpkg/dpkg-maintscript-helper.1.en.html)
- [debhelper-generated autoscripts](https://sources.debian.org/src/debhelper/)

The lifecycle surface consists of `preinst`, `postinst`, `prerm`, `postrm`,
declared triggers, dpkg-provided arguments/environment, and helper-generated
script fragments.

`dpkg-maintscript-helper/v1` recognizes only the documented form:

```text
dpkg-maintscript-helper ACTION ACTION_ARGS [PRIOR_VERSION [PACKAGE]] -- "$@"
```

The four actions are `rm_conffile`, `mv_conffile`, `symlink_to_dir`, and
`dir_to_symlink`. The command, action arguments, separator, and final forwarded
maintainer-script argv must have the expected AST provenance. Package names and
absolute managed paths are validated; symlink targets may be absolute or
relative as dpkg documents.

Current native equivalence:

| Action | Status | Native model |
| --- | --- | --- |
| `rm_conffile` | complete when the path is absent from the new payload | generation `/etc` three-way merge removes an unchanged obsolete config and preserves a user-modified orphan |
| `mv_conffile` | typed partial | needs persisted config-path identity migration so user edits follow the rename |
| `symlink_to_dir` | typed partial | needs explicit customized-symlink collision and rollback semantics |
| `dir_to_symlink` | typed partial | needs owned-directory/content validation, transition staging, and rollback semantics |

Partial entries name the missing native model. They are engineering inputs, not
requests for an operator to reinterpret the script.

## Arch Surface

Authoritative upstream references:

- [ALPM install-scriptlet lifecycle functions](https://man.archlinux.org/man/alpm-install-scriptlet.5.en)
- [ALPM hook file contract](https://man.archlinux.org/man/alpm-hooks.5.en)

The package-level surface is the six documented `.INSTALL` lifecycle
functions plus ALPM hook trigger/action metadata. Function bodies using the
configured shell interpreter enter the formal shell parser with their
lifecycle function retained. Hook files are parsed as ALPM metadata, not
inferred from shell-like strings.

## Residual Programs

Arbitrary shell is intentionally not represented as a finite allowlist.
Unmatched AST command nodes and unsupported language constructs remain typed
residual evidence with lifecycle, provenance, and execution context. The Remi
queue persists a privacy-normalized
`conary.remi.scriptlet-evidence-record.v1` for each clustered observation.
That record retains the formal command and argument provenance, execution
context, lifecycle, evidence source, environment names, and pipeline identity
needed to turn a repeated residual into a native adapter without reparsing or
guessing from the cluster label.

Changing redaction or discovery clustering never requires migration or manual
reconciliation. This pre-alpha epoch rebuilds derived queue state from current
conversion records, and queue state never changes publication authority.

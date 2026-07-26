// conary-core/src/packages/deb/dpkg_lifecycle.rs

//! Source-pinned, byte-preserving argv contracts for dpkg lifecycle tools.
//!
//! These parsers describe command syntax and source-documented dispositions.
//! They do not execute a target binary, read a target package database, or
//! grant mutation authority.

mod alternatives;
mod common;
mod dpkg;
mod maintscript_helper;
mod query;

pub use alternatives::{
    AlternativeSelection, AlternativeSelectionMode, AlternativesAction, AlternativesEffect,
    AlternativesExitStatus, AlternativesInvocation, AlternativesOption, AlternativesOptions,
    AlternativesOutput, AlternativesSlave,
};
pub use common::DpkgLifecycleGrammarError;
pub use dpkg::{
    DpkgArchiveBackendAction, DpkgAssertFeature, DpkgAssertionStatus, DpkgComparisonStatus,
    DpkgDisposition, DpkgForceMode, DpkgForceThing, DpkgInvocation, DpkgMutationAction,
    DpkgMutationClass, DpkgOption, DpkgReadOnlyAction, DpkgValidationKind, DpkgValidationStatus,
    VersionRelation,
};
pub use maintscript_helper::{
    DpkgMaintscriptHelperInvocation, MaintainerReplacement, MaintainerScriptPhase,
    MaintscriptHelperAction, MaintscriptHelperOperation, MaintscriptHelperSupportStatus,
};
pub use query::{
    DpkgQueryAction, DpkgQueryExitStatus, DpkgQueryInvocation, DpkgQueryOption, DpkgQueryOptions,
};

/// Debian dpkg source package version whose executable contracts are modeled.
pub const DPKG_LIFECYCLE_VERSION: &str = "1.23.7";

/// One immutable Debian Sources input used to derive a grammar or result table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PinnedDpkgLifecycleSource {
    pub path: &'static str,
    pub source_url: &'static str,
    pub sha256: &'static str,
}

/// Exact implementation and manual inputs used by this module.
pub const PINNED_DPKG_LIFECYCLE_SOURCES: [PinnedDpkgLifecycleSource; 16] = [
    PinnedDpkgLifecycleSource {
        path: "src/dpkg-maintscript-helper.sh",
        source_url: "https://sources.debian.org/data/main/d/dpkg/1.23.7/src/dpkg-maintscript-helper.sh",
        sha256: "5c2c9c5970653cbf08e685aafe1e3c522a996ad182ce68f5258408f7d114c408",
    },
    PinnedDpkgLifecycleSource {
        path: "man/dpkg-maintscript-helper.pod",
        source_url: "https://sources.debian.org/data/main/d/dpkg/1.23.7/man/dpkg-maintscript-helper.pod",
        sha256: "bdc26ca0d1dfbe19b1bf4b920ecd05348e947038f73c46637c116a08cacf0ba9",
    },
    PinnedDpkgLifecycleSource {
        path: "man/deb-preinst.pod",
        source_url: "https://sources.debian.org/data/main/d/dpkg/1.23.7/man/deb-preinst.pod",
        sha256: "61581febd9997d549b0d01b81c31209ab8b2158cf5edb9177621b0708067ae79",
    },
    PinnedDpkgLifecycleSource {
        path: "man/deb-postinst.pod",
        source_url: "https://sources.debian.org/data/main/d/dpkg/1.23.7/man/deb-postinst.pod",
        sha256: "19cb11bc4e9698f30baae76d4d4718eebcacf9777f8456e21a9751e4e40a71a8",
    },
    PinnedDpkgLifecycleSource {
        path: "man/deb-prerm.pod",
        source_url: "https://sources.debian.org/data/main/d/dpkg/1.23.7/man/deb-prerm.pod",
        sha256: "d416867fcb7af8c840fa38b7843fd5eb857a2d9bedc237852cde5c955e8824cf",
    },
    PinnedDpkgLifecycleSource {
        path: "man/deb-postrm.pod",
        source_url: "https://sources.debian.org/data/main/d/dpkg/1.23.7/man/deb-postrm.pod",
        sha256: "10da90d3159864b9bbc2f9c5e9bd21b3a52f9169e6d4706df831aa4141d765a4",
    },
    PinnedDpkgLifecycleSource {
        path: "src/query/main.c",
        source_url: "https://sources.debian.org/data/main/d/dpkg/1.23.7/src/query/main.c",
        sha256: "40f1d6dec6a3554ad0c1617b584921cfb5b2e0a8b3669e689772c40f6245be80",
    },
    PinnedDpkgLifecycleSource {
        path: "man/dpkg-query.pod",
        source_url: "https://sources.debian.org/data/main/d/dpkg/1.23.7/man/dpkg-query.pod",
        sha256: "5880e02cf076911a2ecbe909a8767565b5ed7a28c986ab3336bd65fd3cbe6374",
    },
    PinnedDpkgLifecycleSource {
        path: "lib/dpkg/options.c",
        source_url: "https://sources.debian.org/data/main/d/dpkg/1.23.7/lib/dpkg/options.c",
        sha256: "fb3382b885b160d3382e176ad4d5cf47ba1d9addedd3a4c67095008eed46ada4",
    },
    PinnedDpkgLifecycleSource {
        path: "src/main/main.c",
        source_url: "https://sources.debian.org/data/main/d/dpkg/1.23.7/src/main/main.c",
        sha256: "a5a5f0ab9a9b6f6c510d25e71e4618add60077b70c01b0bae0d44432ec75ada4",
    },
    PinnedDpkgLifecycleSource {
        path: "src/main/unpack.c",
        source_url: "https://sources.debian.org/data/main/d/dpkg/1.23.7/src/main/unpack.c",
        sha256: "8ca212f6901994b6265f36e34a2d83801a441639e158077242b5cf4ec1b5a9b6",
    },
    PinnedDpkgLifecycleSource {
        path: "src/main/enquiry.c",
        source_url: "https://sources.debian.org/data/main/d/dpkg/1.23.7/src/main/enquiry.c",
        sha256: "1a00c02eb9a073e60413d46e9c19d3ce970c118d3cecb43bd5519fecd13e45fd",
    },
    PinnedDpkgLifecycleSource {
        path: "src/main/trigproc.c",
        source_url: "https://sources.debian.org/data/main/d/dpkg/1.23.7/src/main/trigproc.c",
        sha256: "30d7cfc7dbdc3b255d46bc1e93b51663eb5929ab89acc98571a200663644a566",
    },
    PinnedDpkgLifecycleSource {
        path: "man/dpkg.pod",
        source_url: "https://sources.debian.org/data/main/d/dpkg/1.23.7/man/dpkg.pod",
        sha256: "3d69f601d14ef08a1fd3a999c1aec8665f3837906574bf68af2a12cc6754e851",
    },
    PinnedDpkgLifecycleSource {
        path: "utils/update-alternatives.c",
        source_url: "https://sources.debian.org/data/main/d/dpkg/1.23.7/utils/update-alternatives.c",
        sha256: "3988c478576611f51c5a2a9fffeee69cafeea73981052add2a1dfd5c62c0ae7f",
    },
    PinnedDpkgLifecycleSource {
        path: "man/update-alternatives.pod",
        source_url: "https://sources.debian.org/data/main/d/dpkg/1.23.7/man/update-alternatives.pod",
        sha256: "61d994a142fa08136e9b37eeed8bf87d5a6c0cd23125c406c4c9e720c1481bfe",
    },
];

/// A parsed invocation from one of the four lifecycle bridge endpoints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DpkgLifecycleToolInvocation {
    MaintscriptHelper(DpkgMaintscriptHelperInvocation),
    Query(DpkgQueryInvocation),
    Dpkg(DpkgInvocation),
    UpdateAlternatives(AlternativesInvocation),
}

impl DpkgLifecycleToolInvocation {
    /// Parse an exact endpoint identity and byte-exact argv (without argv[0]).
    pub fn parse(command: &[u8], argv: &[Vec<u8>]) -> Result<Self, DpkgLifecycleGrammarError> {
        match command {
            b"dpkg-maintscript-helper" | b"/usr/bin/dpkg-maintscript-helper" => {
                DpkgMaintscriptHelperInvocation::parse(argv).map(Self::MaintscriptHelper)
            }
            b"dpkg-query" | b"/usr/bin/dpkg-query" => {
                DpkgQueryInvocation::parse(argv).map(Self::Query)
            }
            b"dpkg" | b"/usr/bin/dpkg" => DpkgInvocation::parse(argv).map(Self::Dpkg),
            b"update-alternatives" | b"/usr/bin/update-alternatives" => {
                AlternativesInvocation::parse(argv).map(Self::UpdateAlternatives)
            }
            _ => Err(DpkgLifecycleGrammarError::UnknownTool(command.to_vec())),
        }
    }

    pub const fn command(&self) -> &'static [u8] {
        match self {
            Self::MaintscriptHelper(_) => b"/usr/bin/dpkg-maintscript-helper",
            Self::Query(_) => b"/usr/bin/dpkg-query",
            Self::Dpkg(_) => b"/usr/bin/dpkg",
            Self::UpdateAlternatives(_) => b"/usr/bin/update-alternatives",
        }
    }

    /// Render a canonical byte-exact argv accepted by the pinned grammar.
    pub fn argv(&self) -> Vec<Vec<u8>> {
        match self {
            Self::MaintscriptHelper(invocation) => invocation.argv(),
            Self::Query(invocation) => invocation.argv(),
            Self::Dpkg(invocation) => invocation.argv(),
            Self::UpdateAlternatives(invocation) => invocation.argv(),
        }
    }
}

#[cfg(test)]
#[path = "dpkg_lifecycle/tests.rs"]
mod tests;

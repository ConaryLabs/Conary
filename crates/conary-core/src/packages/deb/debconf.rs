// conary-core/src/packages/deb/debconf.rs

//! Exact, persistence-independent debconf protocol and state engine.
//!
//! This module models the protocol spoken by Debian confmodules without
//! invoking a frontend, reading the filesystem, or depending on Conary's
//! database. Callers own serialization of [`DebconfState`] and provide the
//! typed [`DebconfTemplateLoader`] used by `X_LOADTEMPLATEFILE`.

mod engine;
mod protocol;
mod session;
mod state;
mod template;

pub use engine::DebconfEngine;
pub use protocol::{
    DebconfCallerResult, DebconfCommand, DebconfCommandParseError, DebconfExecution,
    DebconfFlagValue, DebconfPriority, DebconfProgressCommand, DebconfProtocolVersion,
    DebconfResponse, DebconfResultCode, parse_argv, parse_command,
};
pub use session::{
    DebconfBlock, DebconfGoOutcome, DebconfPendingQuestion, DebconfProgress,
    DebconfProgressOutcome, DebconfSession, DebconfSessionPolicy, DebconfSessionRequestError,
};
pub use state::{DebconfQuestion, DebconfState, DebconfStateError, DebconfTemplate};
pub use template::{
    DebconfTemplateImportError, DebconfTemplateLoadError, DebconfTemplateLoadErrorKind,
    DebconfTemplateLoader,
};

/// One byte-exact upstream source used to define this implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DebconfSourceAuthority {
    pub role: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
}

/// Upstream authority is deliberately pinned to one debconf implementation
/// version and one specification revision. Changing either requires reviewing
/// the grammar and state transitions, not merely updating these hashes.
pub const DEBCONF_SOURCE_VERSION: &str = "1.5.92";
pub const DEBCONF_PROTOCOL_REVISION: &str =
    "Protocol 2.1, revision 7.1, Debian Policy 4.7.4.1 (2026-03-31)";

pub const DEBCONF_SOURCE_AUTHORITIES: &[DebconfSourceAuthority] = &[
    DebconfSourceAuthority {
        role: "protocol specification",
        url: "https://www.debian.org/doc/packaging-manuals/debconf_specification.html",
        sha256: "4ac8b0e1e9cac5b917d7d395254ac9f255c16fda98cc713ffd6207d3fc0a428d",
    },
    DebconfSourceAuthority {
        role: "server command grammar and dispatch",
        url: "https://sources.debian.org/data/main/d/debconf/1.5.92/Debconf/ConfModule.pm",
        sha256: "8a653a7d5cdf4c46421700de03682b6518b8562d3ecc1872d78722a347e4f6f9",
    },
    DebconfSourceAuthority {
        role: "client return-code and RET handling",
        url: "https://sources.debian.org/data/main/d/debconf/1.5.92/Debconf/Client/ConfModule.pm",
        sha256: "dee1cb9f961b76d28ef7c6e099ebf0ef4e18bad55f4d0bf843245f8ab2b80457",
    },
    DebconfSourceAuthority {
        role: "shell caller RET handling",
        url: "https://sources.debian.org/data/main/d/debconf/1.5.92/confmodule",
        sha256: "5b043ed174b714949cfb2097d303147b9f02c5fb9e531e2dfb562b1476e820af",
    },
    DebconfSourceAuthority {
        role: "question values flags substitutions and ownership",
        url: "https://sources.debian.org/data/main/d/debconf/1.5.92/Debconf/Question.pm",
        sha256: "4f57bd230d5bf0bed6ffb69ed3b8061bd0f37ecadc1d1a40d2bfb9674ca6c27e",
    },
    DebconfSourceAuthority {
        role: "database ownership contract",
        url: "https://sources.debian.org/data/main/d/debconf/1.5.92/Debconf/DbDriver.pm",
        sha256: "ec54674dacb310aef339511ca7f7467f5c01928bd624901ea2e7db2a15e31015",
    },
    DebconfSourceAuthority {
        role: "database owner removal and field defaults",
        url: "https://sources.debian.org/data/main/d/debconf/1.5.92/Debconf/DbDriver/Cache.pm",
        sha256: "40ec887fb4a8a02c34777f95783d07bc015850404bafdf35fcbda37701fe9ea0",
    },
    DebconfSourceAuthority {
        role: "template import and ownership",
        url: "https://sources.debian.org/data/main/d/debconf/1.5.92/Debconf/Template.pm",
        sha256: "62bc3bfbb15e103d03c7b7d23924ba7edac56f4502a2d71c651b32d5bd926650",
    },
    DebconfSourceAuthority {
        role: "priority grammar and ordering",
        url: "https://sources.debian.org/data/main/d/debconf/1.5.92/Debconf/Priority.pm",
        sha256: "d82b2c2e08d05235ba81ff883170ccc177381fa313262f0f4061e9d046bd49ec",
    },
    DebconfSourceAuthority {
        role: "frontend queue and go transitions",
        url: "https://sources.debian.org/data/main/d/debconf/1.5.92/Debconf/FrontEnd.pm",
        sha256: "7196e9ad50915ffc863eb8259bf3eac0f66487c4dd0ee72e24b9c51e41c19ccc",
    },
    DebconfSourceAuthority {
        role: "noninteractive frontend",
        url: "https://sources.debian.org/data/main/d/debconf/1.5.92/Debconf/FrontEnd/Noninteractive.pm",
        sha256: "9af404289fbbfe6e7e39e062e5a18ef9e8f312392879e7d83be61a152439a65e",
    },
    DebconfSourceAuthority {
        role: "noninteractive element defaults",
        url: "https://sources.debian.org/data/main/d/debconf/1.5.92/Debconf/Element/Noninteractive.pm",
        sha256: "016a5b556bc06cc9371758c11283f61f684da196e3b6582f335a953cec7f7c35",
    },
    DebconfSourceAuthority {
        role: "noninteractive select normalization",
        url: "https://sources.debian.org/data/main/d/debconf/1.5.92/Debconf/Element/Noninteractive/Select.pm",
        sha256: "19f73bca1a336facebb545c2f29860c8b2b60b97e86116e3ffd103e08c950904",
    },
    DebconfSourceAuthority {
        role: "noninteractive error value behavior",
        url: "https://sources.debian.org/data/main/d/debconf/1.5.92/Debconf/Element/Noninteractive/Error.pm",
        sha256: "1e98da2c2ff3fb9b62ad2be9b4b6841818ecbddb4413ae5ca1e5700538e639ab",
    },
    DebconfSourceAuthority {
        role: "noninteractive progress behavior",
        url: "https://sources.debian.org/data/main/d/debconf/1.5.92/Debconf/Element/Noninteractive/Progress.pm",
        sha256: "07df6010430dc2dddf7511e716e9f136609ff9fcba11d6affba6596921427e05",
    },
    DebconfSourceAuthority {
        role: "default priority and noninteractive seen policy",
        url: "https://sources.debian.org/data/main/d/debconf/1.5.92/Debconf/Config.pm",
        sha256: "385e7ae4b6e16c7e904bb8a5ec96158a87458246924e3a87bb79494b8b8f846c",
    },
];

#[cfg(test)]
mod tests;

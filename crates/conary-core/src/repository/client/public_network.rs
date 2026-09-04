// crates/conary-core/src/repository/client/public_network.rs

//! Public-address enforcement and one-resolution HTTP client pinning.

use std::net::{IpAddr, SocketAddr};

use reqwest::header::{HeaderMap, LOCATION};
use reqwest::{Client, Response, StatusCode};
use url::Host;

use super::TimeoutConfig;
use crate::error::{Error, Result};
use crate::repository::error_helpers::http_client_builder_error_message;

const MAX_PUBLIC_REDIRECTS: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NonGlobalRepositoryAddress {
    Unspecified,
    Loopback,
    Private,
    Shared,
    LinkLocal,
    Documentation,
    Benchmarking,
    Multicast,
    Reserved,
    UniqueLocal,
    SiteLocal,
    Ipv4Mapped,
    Ipv4Compatible,
    ProtocolAssignment,
    Translation,
    DiscardOnly,
    SixToFour,
    SixToFourRelay,
    SegmentRouting,
}

impl NonGlobalRepositoryAddress {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Unspecified => "unspecified",
            Self::Loopback => "loopback",
            Self::Private => "private",
            Self::Shared => "shared address space",
            Self::LinkLocal => "link-local",
            Self::Documentation => "documentation",
            Self::Benchmarking => "benchmarking",
            Self::Multicast => "multicast",
            Self::Reserved => "reserved",
            Self::UniqueLocal => "unique-local",
            Self::SiteLocal => "site-local",
            Self::Ipv4Mapped => "IPv4-mapped",
            Self::Ipv4Compatible => "IPv4-compatible",
            Self::ProtocolAssignment => "protocol-assignment",
            Self::Translation => "translation",
            Self::DiscardOnly => "discard-only",
            Self::SixToFour => "6to4",
            Self::SixToFourRelay => "6to4-relay",
            Self::SegmentRouting => "segment-routing",
        }
    }
}

#[derive(Clone, Copy)]
struct Ipv4Range {
    network: u32,
    prefix: u8,
    class: NonGlobalRepositoryAddress,
}

#[derive(Clone, Copy)]
struct Ipv6Range {
    network: u128,
    prefix: u8,
    class: NonGlobalRepositoryAddress,
}

const IPV4_NON_GLOBAL_RANGES: &[Ipv4Range] = &[
    ipv4_range([0, 0, 0, 0], 8, NonGlobalRepositoryAddress::Unspecified),
    ipv4_range([10, 0, 0, 0], 8, NonGlobalRepositoryAddress::Private),
    ipv4_range([100, 64, 0, 0], 10, NonGlobalRepositoryAddress::Shared),
    ipv4_range([127, 0, 0, 0], 8, NonGlobalRepositoryAddress::Loopback),
    ipv4_range([169, 254, 0, 0], 16, NonGlobalRepositoryAddress::LinkLocal),
    ipv4_range([172, 16, 0, 0], 12, NonGlobalRepositoryAddress::Private),
    ipv4_range(
        [192, 0, 0, 0],
        24,
        NonGlobalRepositoryAddress::ProtocolAssignment,
    ),
    ipv4_range(
        [192, 0, 2, 0],
        24,
        NonGlobalRepositoryAddress::Documentation,
    ),
    ipv4_range(
        [192, 88, 99, 0],
        24,
        NonGlobalRepositoryAddress::SixToFourRelay,
    ),
    ipv4_range([192, 168, 0, 0], 16, NonGlobalRepositoryAddress::Private),
    ipv4_range(
        [198, 18, 0, 0],
        15,
        NonGlobalRepositoryAddress::Benchmarking,
    ),
    ipv4_range(
        [198, 51, 100, 0],
        24,
        NonGlobalRepositoryAddress::Documentation,
    ),
    ipv4_range(
        [203, 0, 113, 0],
        24,
        NonGlobalRepositoryAddress::Documentation,
    ),
    ipv4_range([224, 0, 0, 0], 4, NonGlobalRepositoryAddress::Multicast),
    ipv4_range([240, 0, 0, 0], 4, NonGlobalRepositoryAddress::Reserved),
];

const IPV6_NON_GLOBAL_RANGES: &[Ipv6Range] = &[
    ipv6_range(
        0x0064_ff9b_0001_0000_0000_0000_0000_0000,
        48,
        NonGlobalRepositoryAddress::Translation,
    ),
    ipv6_range(
        0x0100_0000_0000_0000_0000_0000_0000_0000,
        64,
        NonGlobalRepositoryAddress::DiscardOnly,
    ),
    ipv6_range(
        0x2001_0002_0000_0000_0000_0000_0000_0000,
        48,
        NonGlobalRepositoryAddress::Benchmarking,
    ),
    ipv6_range(
        0x2001_0000_0000_0000_0000_0000_0000_0000,
        23,
        NonGlobalRepositoryAddress::ProtocolAssignment,
    ),
    ipv6_range(
        0x2002_0000_0000_0000_0000_0000_0000_0000,
        16,
        NonGlobalRepositoryAddress::SixToFour,
    ),
    ipv6_range(
        0x2001_0db8_0000_0000_0000_0000_0000_0000,
        32,
        NonGlobalRepositoryAddress::Documentation,
    ),
    ipv6_range(
        0x3fff_0000_0000_0000_0000_0000_0000_0000,
        20,
        NonGlobalRepositoryAddress::Documentation,
    ),
    ipv6_range(
        0x5f00_0000_0000_0000_0000_0000_0000_0000,
        16,
        NonGlobalRepositoryAddress::SegmentRouting,
    ),
    ipv6_range(
        0xfc00_0000_0000_0000_0000_0000_0000_0000,
        7,
        NonGlobalRepositoryAddress::UniqueLocal,
    ),
    ipv6_range(
        0xfe80_0000_0000_0000_0000_0000_0000_0000,
        10,
        NonGlobalRepositoryAddress::LinkLocal,
    ),
    ipv6_range(
        0xfec0_0000_0000_0000_0000_0000_0000_0000,
        10,
        NonGlobalRepositoryAddress::SiteLocal,
    ),
    ipv6_range(
        0xff00_0000_0000_0000_0000_0000_0000_0000,
        8,
        NonGlobalRepositoryAddress::Multicast,
    ),
];

const fn ipv4_range(octets: [u8; 4], prefix: u8, class: NonGlobalRepositoryAddress) -> Ipv4Range {
    Ipv4Range {
        network: u32::from_be_bytes(octets),
        prefix,
        class,
    }
}

const fn ipv6_range(network: u128, prefix: u8, class: NonGlobalRepositoryAddress) -> Ipv6Range {
    Ipv6Range {
        network,
        prefix,
        class,
    }
}

fn prefix_matches(value: u128, network: u128, width: u8, prefix: u8) -> bool {
    let shift = u32::from(width - prefix);
    value >> shift == network >> shift
}

fn is_global_ietf_protocol_assignment(value: u128) -> bool {
    value == 0x2001_0001_0000_0000_0000_0000_0000_0001
        || value == 0x2001_0001_0000_0000_0000_0000_0000_0002
        || prefix_matches(value, 0x2001_0003_0000_0000_0000_0000_0000_0000, 128, 32)
        || prefix_matches(value, 0x2001_0004_0112_0000_0000_0000_0000_0000, 128, 48)
        || (0x20..=0x3f).contains(&((value >> 96) as u16))
}

fn classify_non_global_repository_ip(ip: IpAddr) -> Option<NonGlobalRepositoryAddress> {
    match ip {
        IpAddr::V4(ip) => {
            let value = u32::from(ip);
            let class = IPV4_NON_GLOBAL_RANGES
                .iter()
                .find(|range| {
                    if range.class == NonGlobalRepositoryAddress::ProtocolAssignment
                        && matches!(ip.octets(), [192, 0, 0, 9 | 10])
                    {
                        return false;
                    }
                    prefix_matches(
                        u128::from(value),
                        u128::from(range.network),
                        32,
                        range.prefix,
                    )
                })
                .map(|range| range.class);
            if class.is_some() {
                return class;
            }
            (!(1..=223).contains(&ip.octets()[0])).then_some(NonGlobalRepositoryAddress::Reserved)
        }
        IpAddr::V6(ip) => {
            if ip.is_unspecified() {
                return Some(NonGlobalRepositoryAddress::Unspecified);
            }
            if ip.is_loopback() {
                return Some(NonGlobalRepositoryAddress::Loopback);
            }
            if ip.to_ipv4_mapped().is_some() {
                return Some(NonGlobalRepositoryAddress::Ipv4Mapped);
            }
            let octets = ip.octets();
            if octets[..12].iter().all(|octet| *octet == 0) {
                return Some(NonGlobalRepositoryAddress::Ipv4Compatible);
            }
            let value = u128::from(ip);
            let class = IPV6_NON_GLOBAL_RANGES
                .iter()
                .find(|range| {
                    if range.class == NonGlobalRepositoryAddress::ProtocolAssignment
                        && is_global_ietf_protocol_assignment(value)
                    {
                        return false;
                    }
                    prefix_matches(value, range.network, 128, range.prefix)
                })
                .map(|range| range.class);
            if class.is_some() {
                return class;
            }
            let globally_routable =
                prefix_matches(value, 0x2000_0000_0000_0000_0000_0000_0000_0000, 128, 3)
                    || prefix_matches(value, 0x0064_ff9b_0000_0000_0000_0000_0000_0000, 128, 96);
            (!globally_routable).then_some(NonGlobalRepositoryAddress::Reserved)
        }
    }
}

/// Validate that a URL uses an allowed scheme (HTTP or HTTPS only).
pub fn validate_url_scheme(url: &str) -> Result<()> {
    if url.starts_with("https://") || url.starts_with("http://") {
        Ok(())
    } else {
        Err(Error::ConfigError(format!(
            "URL must use http:// or https:// scheme: {url}"
        )))
    }
}

pub(crate) fn is_file_or_local_reference(url_or_path: &str) -> bool {
    url_or_path.starts_with("file://") || !has_url_scheme(url_or_path)
}

pub(super) fn has_url_scheme(input: &str) -> bool {
    let Some(colon_index) = input.find(':') else {
        return false;
    };

    let scheme = &input[..colon_index];
    let mut bytes = scheme.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };

    first.is_ascii_alphabetic()
        && bytes.all(|byte| {
            matches!(
                byte,
                b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'+' | b'-' | b'.'
            )
        })
}

pub fn require_public_repository_ip(ip: IpAddr) -> Result<()> {
    if let Some(class) = classify_non_global_repository_ip(ip) {
        return Err(Error::ConfigError(format!(
            "repository URL resolved to non-global {kind} address {ip}",
            kind = class.as_str()
        )));
    }
    Ok(())
}

async fn resolve_repository_addrs<F>(
    host: &str,
    connect_timeout: std::time::Duration,
    resolution: F,
) -> Result<Vec<SocketAddr>>
where
    F: std::future::Future<Output = std::io::Result<Vec<SocketAddr>>>,
{
    tokio::time::timeout(connect_timeout, resolution)
        .await
        .map_err(|_| {
            Error::TimeoutError(format!(
                "repository DNS resolution for '{host}' exceeded the connection timeout"
            ))
        })?
        .map_err(|error| Error::DownloadError(format!("failed to resolve '{host}': {error}")))
}

pub(super) async fn pinned_client_for_url(url: &str, timeouts: &TimeoutConfig) -> Result<Client> {
    let parsed = url::Url::parse(url)
        .map_err(|error| Error::ConfigError(format!("invalid repository URL: {error}")))?;
    let port = parsed
        .port_or_known_default()
        .ok_or_else(|| Error::ConfigError("repository URL has no port".to_string()))?;

    let mut builder = Client::builder()
        .connect_timeout(timeouts.connect)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy();
    match parsed
        .host()
        .ok_or_else(|| Error::ConfigError("repository URL has no host".to_string()))?
    {
        Host::Domain(host) => {
            let resolution = async {
                tokio::net::lookup_host((host, port))
                    .await
                    .map(|addrs| addrs.collect::<Vec<SocketAddr>>())
            };
            let addrs = resolve_repository_addrs(host, timeouts.connect, resolution).await?;
            if addrs.is_empty() {
                return Err(Error::DownloadError(format!(
                    "DNS resolution for '{host}' returned no addresses"
                )));
            }
            for addr in &addrs {
                require_public_repository_ip(addr.ip())?;
            }
            builder = builder.resolve_to_addrs(host, &addrs);
        }
        Host::Ipv4(ip) => require_public_repository_ip(IpAddr::V4(ip))?,
        Host::Ipv6(ip) => require_public_repository_ip(IpAddr::V6(ip))?,
    }

    builder
        .build()
        .map_err(|error| Error::InitError(http_client_builder_error_message(error)))
}

pub(super) async fn get_following_public_redirects(
    url: &str,
    headers: &HeaderMap,
    timeouts: &TimeoutConfig,
    request_timeout: Option<std::time::Duration>,
) -> Result<Response> {
    let mut current = url::Url::parse(url)
        .map_err(|error| Error::ConfigError(format!("invalid repository URL: {error}")))?;
    let original_was_https = current.scheme() == "https";

    for redirects in 0..=MAX_PUBLIC_REDIRECTS {
        validate_public_request_url(&current, original_was_https)?;
        let client = pinned_client_for_url(current.as_str(), timeouts).await?;
        let mut request = client.get(current.clone()).headers(headers.clone());
        if let Some(timeout) = request_timeout {
            request = request.timeout(timeout);
        }
        let response = request.send().await.map_err(|error| {
            Error::DownloadError(format!("Failed to download {current}: {error}"))
        })?;
        if !is_followed_redirect(response.status()) {
            return Ok(response);
        }
        if redirects == MAX_PUBLIC_REDIRECTS {
            return Err(Error::DownloadError(format!(
                "repository URL exceeded {MAX_PUBLIC_REDIRECTS} redirects: {url}"
            )));
        }
        let location = response
            .headers()
            .get(LOCATION)
            .ok_or_else(|| {
                Error::DownloadError(format!(
                    "repository redirect from {current} has no Location header"
                ))
            })?
            .to_str()
            .map_err(|error| {
                Error::DownloadError(format!(
                    "repository redirect from {current} has an invalid Location header: {error}"
                ))
            })?;
        current = current.join(location).map_err(|error| {
            Error::DownloadError(format!(
                "repository redirect from {current} has an invalid target: {error}"
            ))
        })?;
    }
    unreachable!("redirect loop is bounded")
}

fn is_followed_redirect(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::MOVED_PERMANENTLY
            | StatusCode::FOUND
            | StatusCode::SEE_OTHER
            | StatusCode::TEMPORARY_REDIRECT
            | StatusCode::PERMANENT_REDIRECT
    )
}

fn validate_public_request_url(url: &url::Url, original_was_https: bool) -> Result<()> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(Error::ConfigError(format!(
            "repository redirect target must use http:// or https://: {url}"
        )));
    }
    if original_was_https && url.scheme() != "https" {
        return Err(Error::ConfigError(format!(
            "repository redirect may not downgrade HTTPS to HTTP: {url}"
        )));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(Error::ConfigError(
            "repository URLs may not contain credentials".to_string(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_every_forbidden_repository_address_family() {
        let cases = [
            ("0.0.0.0", NonGlobalRepositoryAddress::Unspecified),
            ("127.0.0.1", NonGlobalRepositoryAddress::Loopback),
            ("10.0.0.1", NonGlobalRepositoryAddress::Private),
            ("100.64.0.1", NonGlobalRepositoryAddress::Shared),
            ("169.254.1.1", NonGlobalRepositoryAddress::LinkLocal),
            ("192.0.2.1", NonGlobalRepositoryAddress::Documentation),
            ("198.18.0.1", NonGlobalRepositoryAddress::Benchmarking),
            ("224.0.0.1", NonGlobalRepositoryAddress::Multicast),
            ("240.0.0.1", NonGlobalRepositoryAddress::Reserved),
            ("192.88.99.1", NonGlobalRepositoryAddress::SixToFourRelay),
            ("::", NonGlobalRepositoryAddress::Unspecified),
            ("::1", NonGlobalRepositoryAddress::Loopback),
            ("fc00::1", NonGlobalRepositoryAddress::UniqueLocal),
            ("fe80::1", NonGlobalRepositoryAddress::LinkLocal),
            ("fec0::1", NonGlobalRepositoryAddress::SiteLocal),
            ("2001:db8::1", NonGlobalRepositoryAddress::Documentation),
            ("2001:2::1", NonGlobalRepositoryAddress::Benchmarking),
            (
                "2001:100::1",
                NonGlobalRepositoryAddress::ProtocolAssignment,
            ),
            ("2002::1", NonGlobalRepositoryAddress::SixToFour),
            ("64:ff9b:1::1", NonGlobalRepositoryAddress::Translation),
            ("100::1", NonGlobalRepositoryAddress::DiscardOnly),
            ("5f00::1", NonGlobalRepositoryAddress::SegmentRouting),
            ("ff02::1", NonGlobalRepositoryAddress::Multicast),
            ("::ffff:8.8.8.8", NonGlobalRepositoryAddress::Ipv4Mapped),
            ("::8.8.8.8", NonGlobalRepositoryAddress::Ipv4Compatible),
            ("4000::1", NonGlobalRepositoryAddress::Reserved),
        ];

        for (address, expected) in cases {
            let ip = address.parse().expect("valid test IP address");
            assert_eq!(
                classify_non_global_repository_ip(ip),
                Some(expected),
                "wrong classification for {address}"
            );
            assert!(
                require_public_repository_ip(ip).is_err(),
                "allowed {address}"
            );
        }
    }

    #[test]
    fn accepts_global_unicast_repository_addresses() {
        for address in [
            "8.8.8.8",
            "1.1.1.1",
            "192.0.0.9",
            "192.0.0.10",
            "2001:4860:4860::8888",
            "2606:4700:4700::1111",
            "2001:1::1",
            "2001:1::2",
            "2001:3::1",
            "2001:4:112::1",
            "2001:20::1",
        ] {
            let ip = address.parse().expect("valid test IP address");
            assert_eq!(
                classify_non_global_repository_ip(ip),
                None,
                "rejected {address}"
            );
            require_public_repository_ip(ip).expect("global unicast address");
        }
    }

    #[test]
    fn redirect_targets_preserve_public_https_authority() {
        let base = url::Url::parse("https://archlinux.org/packages/download/").unwrap();
        let target = base
            .join("https://geo.mirror.pkgbuild.com/core/keyring.pkg.tar.zst")
            .unwrap();
        validate_public_request_url(&target, true).expect("public HTTPS redirect");

        let downgrade = base.join("http://mirror.example/keyring").unwrap();
        assert!(validate_public_request_url(&downgrade, true).is_err());
        let local_file = base.join("file:///etc/passwd").unwrap();
        assert!(validate_public_request_url(&local_file, true).is_err());
        let credentials = base
            .join("https://user:secret@example.test/keyring")
            .unwrap();
        assert!(validate_public_request_url(&credentials, true).is_err());
    }

    #[tokio::test]
    async fn stalled_dns_resolution_uses_the_connection_timeout() {
        let error = resolve_repository_addrs(
            "stalled.example",
            std::time::Duration::from_millis(1),
            std::future::pending::<std::io::Result<Vec<SocketAddr>>>(),
        )
        .await
        .expect_err("a stalled resolver must time out");

        assert!(
            matches!(&error, Error::TimeoutError(message) if message.contains("stalled.example")),
            "unexpected resolver error: {error}"
        );
    }
}

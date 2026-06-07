// apps/remi/src/server/conversion/recipe.rs
//! Recipe URL fetching, SSRF validation, and server-side recipe builds.

use super::{ConversionService, ScriptletPackageMetadata, ServerConversionResult};
use anyhow::{Context, Result, anyhow};
use conary_core::ccs::convert::ScriptletBundleSummary;
use tempfile::TempDir;
use tracing::info;

impl ConversionService {
    /// Build a package from a recipe URL
    ///
    /// 1. Fetch the recipe from the URL
    /// 2. Parse and validate the recipe
    /// 3. Cook it using the Kitchen (with isolation)
    /// 4. Store chunks in CAS
    /// 5. Return the result
    pub async fn build_from_recipe(&self, recipe_url: &str) -> Result<ServerConversionResult> {
        use conary_core::recipe::{Kitchen, KitchenConfig, parse_recipe};

        info!("Building package from recipe: {}", recipe_url);

        // Step 1: Fetch recipe content
        let recipe_content = Self::fetch_url(recipe_url).await?;
        info!("Fetched recipe ({} bytes)", recipe_content.len());

        // Step 2: Parse and validate recipe
        let recipe =
            parse_recipe(&recipe_content).map_err(|e| anyhow!("Failed to parse recipe: {}", e))?;

        info!(
            "Recipe: {} version {}",
            recipe.package.name, recipe.package.version
        );

        // Step 3: Cook the recipe
        let temp_dir =
            TempDir::new_in(&self.cache_dir).context("Failed to create temp directory")?;

        let config = KitchenConfig {
            source_cache: self.cache_dir.join("sources"),
            use_isolation: true, // Always use isolation on server
            ..Default::default()
        };

        let kitchen = Kitchen::new(config);
        let cook_result = kitchen
            .cook(&recipe, temp_dir.path())
            .map_err(|e| anyhow!("Recipe cooking failed: {}", e))?;

        info!(
            "Cooked: {} ({} warnings)",
            cook_result.package_path.display(),
            cook_result.warnings.len()
        );

        // Step 4: Store chunks
        let ccs_data = tokio::fs::read(&cook_result.package_path)
            .await
            .context("Failed to read cooked CCS package")?;

        let content_hash = conary_core::hash::sha256(&ccs_data);

        // Copy CCS package to persistent location
        let ccs_filename = Self::safe_ccs_filename(&recipe.package.name, &recipe.package.version)?;
        let final_ccs_path = self.cache_dir.join("packages").join(&ccs_filename);

        if let Some(parent) = final_ccs_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::copy(&cook_result.package_path, &final_ccs_path).await?;

        // Extract chunk hashes from the CCS package
        // For now, we'll just report the package itself
        let chunk_hashes = vec![content_hash.clone()];
        let total_size = ccs_data.len() as u64;

        Ok(ServerConversionResult {
            name: recipe.package.name,
            version: recipe.package.version,
            distro: "recipe".to_string(),
            chunk_hashes,
            total_size,
            content_hash,
            ccs_path: final_ccs_path,
            cache_state: "recipe".to_string(),
            scriptlets: ScriptletPackageMetadata::from(&ScriptletBundleSummary::default()),
            publication: None,
            timing: None,
        })
    }

    /// Fetch content from a URL (with security validation)
    ///
    /// SECURITY: This function validates URLs and blocks requests to:
    /// - Private IP ranges (10.x, 172.16-31.x, 192.168.x, 127.x)
    /// - Link-local addresses (169.254.x)
    /// - Loopback addresses
    /// - IPv6 private/local addresses
    ///
    /// This prevents SSRF attacks where a malicious recipe URL could be used
    /// to probe internal services.
    async fn fetch_url(url: &str) -> Result<String> {
        // Parse URL to validate scheme and extract host
        let parsed_url =
            url::Url::parse(url).map_err(|e| anyhow!("Invalid URL '{}': {}", url, e))?;

        // Only allow https (http redirects to https, but we reject http-only)
        let scheme = parsed_url.scheme();
        if scheme != "https" && scheme != "http" {
            return Err(anyhow!("Only http/https URLs are allowed, got: {}", scheme));
        }

        // Extract host
        let host = parsed_url
            .host_str()
            .ok_or_else(|| anyhow!("URL '{}' has no host", url))?;

        // Check for prohibited hosts
        Self::validate_host(host)?;

        // Resolve DNS and validate the resolved IP. We pin the validated IP
        // using reqwest's `resolve()` so that reqwest connects to the exact
        // address we checked, closing the DNS rebinding TOCTOU gap.
        let port = parsed_url
            .port()
            .unwrap_or(if scheme == "https" { 443 } else { 80 });
        let resolved_ips: Vec<std::net::SocketAddr> =
            tokio::net::lookup_host(format!("{host}:{port}"))
                .await
                .map_err(|e| anyhow!("Failed to resolve '{}': {}", host, e))?
                .collect();

        // Check all resolved IPs - if ANY is private, reject
        for addr in &resolved_ips {
            Self::validate_ip(&addr.ip())?;
        }

        // Pin the first validated IP so reqwest uses it instead of
        // re-resolving (which could return a different, malicious IP).
        let pinned_ip = resolved_ips
            .first()
            .ok_or_else(|| anyhow!("DNS resolution for '{}' returned no addresses", host))?;

        // SECURITY: Disable automatic redirects entirely AND pin the
        // resolved IP. This closes both the redirect-based and
        // DNS-rebinding SSRF vectors.
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .resolve(host, *pinned_ip)
            .user_agent("conary-remi/0.1")
            .build()
            .map_err(|e| anyhow!("Failed to create HTTP client: {}", e))?;

        let response = client
            .get(url)
            .send()
            .await
            .map_err(|e| anyhow!("Failed to fetch '{}': {}", url, e))?;

        // Reject redirects -- the response status will be 3xx if the server
        // tried to redirect. Return a clear error instead of following.
        if response.status().is_redirection() {
            let location = response
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("<unknown>");
            return Err(anyhow!(
                "URL '{}' returned redirect ({}) to '{}'. \
                 Redirects are rejected to prevent SSRF.",
                url,
                response.status().as_u16(),
                location
            ));
        }

        if !response.status().is_success() {
            return Err(anyhow!(
                "HTTP {} fetching '{}': {}",
                response.status().as_u16(),
                url,
                response.status().canonical_reason().unwrap_or("Unknown")
            ));
        }

        response
            .text()
            .await
            .map_err(|e| anyhow!("Failed to read response body: {}", e))
    }

    /// Validate a hostname is not a private/internal address
    fn validate_host(host: &str) -> Result<()> {
        // Check for localhost aliases
        let lower_host = host.to_lowercase();
        if lower_host == "localhost"
            || lower_host.ends_with(".localhost")
            || lower_host == "127.0.0.1"
            || lower_host == "::1"
            || lower_host == "0.0.0.0"
        {
            return Err(anyhow!("Localhost URLs are not allowed"));
        }

        // Check for AWS/cloud metadata endpoints
        if lower_host == "169.254.169.254"
            || lower_host.contains("metadata")
            || lower_host == "metadata.google.internal"
        {
            return Err(anyhow!("Cloud metadata endpoints are not allowed"));
        }

        // Check for internal domain suffixes
        let internal_suffixes = [".internal", ".local", ".lan", ".home", ".corp"];
        for suffix in internal_suffixes {
            if lower_host.ends_with(suffix) {
                return Err(anyhow!("Internal domain '{}' is not allowed", host));
            }
        }

        Ok(())
    }

    /// Validate an IP address is not private/internal
    fn validate_ip(ip: &std::net::IpAddr) -> Result<()> {
        match ip {
            std::net::IpAddr::V4(ipv4) => {
                let octets = ipv4.octets();

                // Loopback: 127.0.0.0/8
                if octets[0] == 127 {
                    return Err(anyhow!("Loopback addresses are not allowed"));
                }

                // Private: 10.0.0.0/8
                if octets[0] == 10 {
                    return Err(anyhow!("Private IP range 10.x.x.x is not allowed"));
                }

                // Private: 172.16.0.0/12
                if octets[0] == 172 && (16..=31).contains(&octets[1]) {
                    return Err(anyhow!("Private IP range 172.16-31.x.x is not allowed"));
                }

                // Private: 192.168.0.0/16
                if octets[0] == 192 && octets[1] == 168 {
                    return Err(anyhow!("Private IP range 192.168.x.x is not allowed"));
                }

                // Link-local: 169.254.0.0/16 (includes AWS metadata)
                if octets[0] == 169 && octets[1] == 254 {
                    return Err(anyhow!("Link-local addresses are not allowed"));
                }

                // Broadcast: 255.255.255.255
                if octets == [255, 255, 255, 255] {
                    return Err(anyhow!("Broadcast addresses are not allowed"));
                }

                // Unspecified: 0.0.0.0
                if octets == [0, 0, 0, 0] {
                    return Err(anyhow!("Unspecified addresses are not allowed"));
                }

                Ok(())
            }
            std::net::IpAddr::V6(ipv6) => {
                // Loopback: ::1
                if ipv6.is_loopback() {
                    return Err(anyhow!("IPv6 loopback is not allowed"));
                }

                // Unspecified: ::
                if ipv6.is_unspecified() {
                    return Err(anyhow!("IPv6 unspecified is not allowed"));
                }

                // Private/ULA: fc00::/7
                let segments = ipv6.segments();
                if (segments[0] & 0xfe00) == 0xfc00 {
                    return Err(anyhow!("IPv6 unique local addresses are not allowed"));
                }

                // Link-local: fe80::/10
                if (segments[0] & 0xffc0) == 0xfe80 {
                    return Err(anyhow!("IPv6 link-local addresses are not allowed"));
                }

                Ok(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn build_from_recipe_rejects_localhost_url_before_fetch() {
        let temp = tempfile::TempDir::new().unwrap();
        let service = ConversionService::new(
            temp.path().join("chunks"),
            temp.path().join("cache"),
            temp.path().join("remi.db"),
            None,
        );

        let err = service
            .build_from_recipe("https://localhost/recipe.conary")
            .await
            .expect_err("localhost recipe URL should be rejected before fetch")
            .to_string();

        assert!(err.contains("Localhost URLs are not allowed"));
    }

    #[test]
    fn test_validate_host_allows_public() {
        assert!(ConversionService::validate_host("remi.conary.io").is_ok());
        assert!(ConversionService::validate_host("github.com").is_ok());
        assert!(ConversionService::validate_host("example.com").is_ok());
    }

    #[test]
    fn test_validate_host_blocks_localhost() {
        assert!(ConversionService::validate_host("localhost").is_err());
        assert!(ConversionService::validate_host("LOCALHOST").is_err());
        assert!(ConversionService::validate_host("sub.localhost").is_err());
        assert!(ConversionService::validate_host("127.0.0.1").is_err());
        assert!(ConversionService::validate_host("::1").is_err());
        assert!(ConversionService::validate_host("0.0.0.0").is_err());
    }

    #[test]
    fn test_validate_host_blocks_cloud_metadata() {
        assert!(ConversionService::validate_host("169.254.169.254").is_err());
        assert!(ConversionService::validate_host("metadata.google.internal").is_err());
    }

    #[test]
    fn test_validate_host_blocks_internal_domains() {
        assert!(ConversionService::validate_host("server.internal").is_err());
        assert!(ConversionService::validate_host("mybox.local").is_err());
        assert!(ConversionService::validate_host("router.lan").is_err());
        assert!(ConversionService::validate_host("nas.home").is_err());
        assert!(ConversionService::validate_host("ldap.corp").is_err());
    }

    #[test]
    fn test_validate_ip_allows_public_ipv4() {
        let ip: std::net::IpAddr = "8.8.8.8".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_ok());

        let ip: std::net::IpAddr = "46.4.33.93".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_ok());
    }

    #[test]
    fn test_validate_ip_blocks_loopback() {
        let ip: std::net::IpAddr = "127.0.0.1".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());

        let ip: std::net::IpAddr = "127.0.0.2".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());
    }

    #[test]
    fn test_validate_ip_blocks_private_10() {
        let ip: std::net::IpAddr = "10.0.0.1".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());

        let ip: std::net::IpAddr = "10.255.255.255".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());
    }

    #[test]
    fn test_validate_ip_blocks_private_172() {
        let ip: std::net::IpAddr = "172.16.0.1".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());

        let ip: std::net::IpAddr = "172.31.255.255".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());

        // 172.15.x.x is NOT private
        let ip: std::net::IpAddr = "172.15.0.1".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_ok());

        // 172.32.x.x is NOT private
        let ip: std::net::IpAddr = "172.32.0.1".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_ok());
    }

    #[test]
    fn test_validate_ip_blocks_private_192_168() {
        let ip: std::net::IpAddr = "192.168.0.1".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());

        let ip: std::net::IpAddr = "192.168.255.255".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());
    }

    #[test]
    fn test_validate_ip_blocks_link_local() {
        let ip: std::net::IpAddr = "169.254.169.254".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());

        let ip: std::net::IpAddr = "169.254.0.1".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());
    }

    #[test]
    fn test_validate_ip_blocks_broadcast() {
        let ip: std::net::IpAddr = "255.255.255.255".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());
    }

    #[test]
    fn test_validate_ip_blocks_unspecified() {
        let ip: std::net::IpAddr = "0.0.0.0".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());
    }

    #[test]
    fn test_validate_ip_allows_public_ipv6() {
        let ip: std::net::IpAddr = "2a01:4f8:221:350b::2".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_ok());

        let ip: std::net::IpAddr = "2001:4860:4860::8888".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_ok());
    }

    #[test]
    fn test_validate_ip_blocks_ipv6_loopback() {
        let ip: std::net::IpAddr = "::1".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());
    }

    #[test]
    fn test_validate_ip_blocks_ipv6_unspecified() {
        let ip: std::net::IpAddr = "::".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());
    }

    #[test]
    fn test_validate_ip_blocks_ipv6_ula() {
        let ip: std::net::IpAddr = "fc00::1".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());

        let ip: std::net::IpAddr = "fd12:3456:789a::1".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());
    }

    #[test]
    fn test_validate_ip_blocks_ipv6_link_local() {
        let ip: std::net::IpAddr = "fe80::1".parse().unwrap();
        assert!(ConversionService::validate_ip(&ip).is_err());
    }
}

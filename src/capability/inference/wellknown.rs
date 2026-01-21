// src/capability/inference/wellknown.rs
//! Well-known capability profiles for common packages
//!
//! This module contains pre-defined capability profiles for well-known packages
//! like nginx, postgresql, apache, etc. These profiles are curated and high-confidence
//! since they're based on extensive knowledge of how these packages operate.
//!
//! This is Tier 1 of inference - the fastest and most reliable.

use super::confidence::{Confidence, ConfidenceScore};
use super::{InferredCapabilities, InferredFilesystem, InferredNetwork, InferenceSource};
use std::collections::HashMap;
use std::sync::LazyLock;

/// Registry of well-known package profiles
pub struct WellKnownProfiles;

impl WellKnownProfiles {
    /// Look up a well-known profile by package name
    pub fn lookup(package_name: &str) -> Option<InferredCapabilities> {
        // Try exact match first
        if let Some(profile) = PROFILES.get(package_name) {
            return Some(profile.clone());
        }

        // Try matching base name (strip version suffixes like nginx1.24)
        let base_name = strip_version_suffix(package_name);
        if base_name != package_name
            && let Some(profile) = PROFILES.get(base_name)
        {
            return Some(profile.clone());
        }

        // Try matching by prefix for versioned packages (e.g., python3.11 -> python)
        for (name, profile) in PROFILES.iter() {
            if package_name.starts_with(name) && package_name.len() > name.len() {
                let suffix = &package_name[name.len()..];
                // Check if suffix looks like a version (starts with digit or -)
                if suffix.starts_with(|c: char| c.is_ascii_digit() || c == '-') {
                    return Some(profile.clone());
                }
            }
        }

        None
    }

    /// Check if a package has a well-known profile
    pub fn has_profile(package_name: &str) -> bool {
        Self::lookup(package_name).is_some()
    }

    /// List all known package names (for debugging/completeness checking)
    pub fn list_known_packages() -> Vec<&'static str> {
        PROFILES.keys().copied().collect()
    }
}

/// Strip version suffix from package name
fn strip_version_suffix(name: &str) -> &str {
    // Handle patterns like "nginx-1.24" or "python3.11"
    if let Some(pos) = name.rfind(|c: char| c == '-' || (c.is_ascii_digit() && name.contains('.'))) {
        // Check if everything after is version-like
        let suffix = &name[pos..];
        if suffix.chars().skip(1).all(|c| c.is_ascii_digit() || c == '.') {
            return &name[..pos];
        }
    }
    name
}

/// Create a profile for a network server
fn network_server_profile(
    name: &str,
    listen_ports: &[&str],
    read_paths: &[&str],
    write_paths: &[&str],
) -> InferredCapabilities {
    InferredCapabilities {
        network: InferredNetwork {
            listen_ports: listen_ports.iter().map(|s| (*s).to_string()).collect(),
            outbound_ports: Vec::new(),
            no_network: false,
            confidence: Confidence::High,
        },
        filesystem: InferredFilesystem {
            read_paths: read_paths.iter().map(|s| (*s).to_string()).collect(),
            write_paths: write_paths.iter().map(|s| (*s).to_string()).collect(),
            execute_paths: Vec::new(),
            confidence: Confidence::High,
        },
        syscall_profile: Some("network-server".to_string()),
        confidence: ConfidenceScore::new(Confidence::High),
        tier_used: 1,
        rationale: format!("Well-known profile for {}", name),
        source: InferenceSource::WellKnown,
    }
}

/// Create a profile for a network client
fn network_client_profile(
    name: &str,
    outbound: &[&str],
    read_paths: &[&str],
    write_paths: &[&str],
) -> InferredCapabilities {
    InferredCapabilities {
        network: InferredNetwork {
            listen_ports: Vec::new(),
            outbound_ports: outbound.iter().map(|s| (*s).to_string()).collect(),
            no_network: false,
            confidence: Confidence::High,
        },
        filesystem: InferredFilesystem {
            read_paths: read_paths.iter().map(|s| (*s).to_string()).collect(),
            write_paths: write_paths.iter().map(|s| (*s).to_string()).collect(),
            execute_paths: Vec::new(),
            confidence: Confidence::High,
        },
        syscall_profile: Some("network-client".to_string()),
        confidence: ConfidenceScore::new(Confidence::High),
        tier_used: 1,
        rationale: format!("Well-known profile for {}", name),
        source: InferenceSource::WellKnown,
    }
}

/// Create a profile for a system daemon
fn daemon_profile(
    name: &str,
    listen_ports: &[&str],
    read_paths: &[&str],
    write_paths: &[&str],
) -> InferredCapabilities {
    InferredCapabilities {
        network: InferredNetwork {
            listen_ports: listen_ports.iter().map(|s| (*s).to_string()).collect(),
            outbound_ports: Vec::new(),
            no_network: listen_ports.is_empty(),
            confidence: Confidence::High,
        },
        filesystem: InferredFilesystem {
            read_paths: read_paths.iter().map(|s| (*s).to_string()).collect(),
            write_paths: write_paths.iter().map(|s| (*s).to_string()).collect(),
            execute_paths: Vec::new(),
            confidence: Confidence::High,
        },
        syscall_profile: Some("system-daemon".to_string()),
        confidence: ConfidenceScore::new(Confidence::High),
        tier_used: 1,
        rationale: format!("Well-known profile for {}", name),
        source: InferenceSource::WellKnown,
    }
}

/// Create a profile for a CLI tool (minimal capabilities)
fn cli_profile(name: &str, read_paths: &[&str], write_paths: &[&str]) -> InferredCapabilities {
    InferredCapabilities {
        network: InferredNetwork {
            listen_ports: Vec::new(),
            outbound_ports: Vec::new(),
            no_network: true,
            confidence: Confidence::High,
        },
        filesystem: InferredFilesystem {
            read_paths: read_paths.iter().map(|s| (*s).to_string()).collect(),
            write_paths: write_paths.iter().map(|s| (*s).to_string()).collect(),
            execute_paths: Vec::new(),
            confidence: Confidence::Medium,
        },
        syscall_profile: Some("minimal".to_string()),
        confidence: ConfidenceScore::new(Confidence::Medium),
        tier_used: 1,
        rationale: format!("Well-known profile for {}", name),
        source: InferenceSource::WellKnown,
    }
}

// Static registry of well-known profiles
static PROFILES: LazyLock<HashMap<&'static str, InferredCapabilities>> = LazyLock::new(|| {
    let mut m = HashMap::new();

    // Web servers
    m.insert(
        "nginx",
        network_server_profile(
            "nginx",
            &["80", "443"],
            &["/etc/nginx", "/etc/ssl/certs", "/usr/share/nginx"],
            &["/var/log/nginx", "/var/cache/nginx", "/run/nginx.pid"],
        ),
    );

    m.insert(
        "apache2",
        network_server_profile(
            "apache2",
            &["80", "443"],
            &["/etc/apache2", "/etc/ssl/certs", "/var/www"],
            &["/var/log/apache2", "/var/cache/apache2", "/run/apache2"],
        ),
    );

    m.insert(
        "httpd",
        network_server_profile(
            "httpd",
            &["80", "443"],
            &["/etc/httpd", "/etc/ssl/certs", "/var/www"],
            &["/var/log/httpd", "/run/httpd"],
        ),
    );

    m.insert(
        "caddy",
        network_server_profile(
            "caddy",
            &["80", "443"],
            &["/etc/caddy", "/etc/ssl/certs"],
            &["/var/log/caddy", "/var/lib/caddy"],
        ),
    );

    // Databases
    m.insert(
        "postgresql",
        network_server_profile(
            "postgresql",
            &["5432"],
            &["/etc/postgresql"],
            &["/var/lib/postgresql", "/var/log/postgresql", "/run/postgresql"],
        ),
    );

    m.insert(
        "postgres",
        network_server_profile(
            "postgres",
            &["5432"],
            &["/etc/postgresql"],
            &["/var/lib/postgresql", "/var/log/postgresql", "/run/postgresql"],
        ),
    );

    m.insert(
        "mysql-server",
        network_server_profile(
            "mysql-server",
            &["3306"],
            &["/etc/mysql"],
            &["/var/lib/mysql", "/var/log/mysql", "/run/mysqld"],
        ),
    );

    m.insert(
        "mariadb-server",
        network_server_profile(
            "mariadb-server",
            &["3306"],
            &["/etc/mysql"],
            &["/var/lib/mysql", "/var/log/mariadb", "/run/mariadb"],
        ),
    );

    m.insert(
        "redis",
        network_server_profile(
            "redis",
            &["6379"],
            &["/etc/redis"],
            &["/var/lib/redis", "/var/log/redis", "/run/redis"],
        ),
    );

    m.insert(
        "redis-server",
        network_server_profile(
            "redis-server",
            &["6379"],
            &["/etc/redis"],
            &["/var/lib/redis", "/var/log/redis", "/run/redis"],
        ),
    );

    m.insert(
        "mongodb",
        network_server_profile(
            "mongodb",
            &["27017"],
            &["/etc/mongodb", "/etc/mongod.conf"],
            &["/var/lib/mongodb", "/var/log/mongodb"],
        ),
    );

    // Message queues
    m.insert(
        "rabbitmq-server",
        network_server_profile(
            "rabbitmq-server",
            &["5672", "15672", "25672"],
            &["/etc/rabbitmq"],
            &["/var/lib/rabbitmq", "/var/log/rabbitmq"],
        ),
    );

    // System services
    m.insert(
        "openssh-server",
        network_server_profile(
            "openssh-server",
            &["22"],
            &["/etc/ssh", "/etc/ssl/certs"],
            &["/var/log/auth.log", "/run/sshd.pid"],
        ),
    );

    m.insert(
        "sshd",
        network_server_profile(
            "sshd",
            &["22"],
            &["/etc/ssh", "/etc/ssl/certs"],
            &["/var/log/auth.log", "/run/sshd.pid"],
        ),
    );

    m.insert(
        "systemd",
        daemon_profile(
            "systemd",
            &[],
            &["/etc/systemd", "/lib/systemd"],
            &["/var/log/journal", "/run/systemd"],
        ),
    );

    m.insert(
        "dbus",
        daemon_profile(
            "dbus",
            &[],
            &["/etc/dbus-1", "/usr/share/dbus-1"],
            &["/run/dbus", "/var/lib/dbus"],
        ),
    );

    m.insert(
        "cron",
        daemon_profile(
            "cron",
            &[],
            &["/etc/cron.d", "/etc/crontab", "/var/spool/cron"],
            &["/var/log/cron.log", "/run/crond.pid"],
        ),
    );

    // Network clients
    m.insert(
        "curl",
        network_client_profile(
            "curl",
            &["80", "443"],
            &["/etc/ssl/certs", "$HOME/.curlrc"],
            &[],
        ),
    );

    m.insert(
        "wget",
        network_client_profile(
            "wget",
            &["80", "443", "21"],
            &["/etc/ssl/certs", "$HOME/.wgetrc"],
            &[],
        ),
    );

    m.insert(
        "git",
        network_client_profile(
            "git",
            &["22", "443", "9418"],
            &["/etc/gitconfig", "$HOME/.gitconfig", "/etc/ssl/certs"],
            &["$HOME/.git-credentials"],
        ),
    );

    // CLI tools
    m.insert(
        "coreutils",
        cli_profile("coreutils", &["/"], &[]),
    );

    m.insert(
        "grep",
        cli_profile("grep", &["/"], &[]),
    );

    m.insert(
        "sed",
        cli_profile("sed", &["/"], &[]),
    );

    m.insert(
        "gawk",
        cli_profile("gawk", &["/"], &[]),
    );

    m.insert(
        "vim",
        cli_profile("vim", &["/", "$HOME/.vimrc"], &["$HOME/.viminfo"]),
    );

    m.insert(
        "nano",
        cli_profile("nano", &["/", "$HOME/.nanorc"], &[]),
    );

    // Build tools
    m.insert(
        "gcc",
        cli_profile(
            "gcc",
            &["/usr/include", "/usr/lib"],
            &["/tmp"],
        ),
    );

    m.insert(
        "make",
        cli_profile("make", &["/"], &["."]),
    );

    m.insert(
        "cmake",
        cli_profile("cmake", &["/usr/share/cmake"], &["."]),
    );

    // Container/virtualization
    m.insert(
        "docker",
        daemon_profile(
            "docker",
            &["2375", "2376"],
            &["/etc/docker", "/var/lib/docker"],
            &["/var/lib/docker", "/var/run/docker.sock"],
        ),
    );

    m.insert(
        "containerd",
        daemon_profile(
            "containerd",
            &[],
            &["/etc/containerd"],
            &["/var/lib/containerd", "/run/containerd"],
        ),
    );

    m.insert(
        "podman",
        daemon_profile(
            "podman",
            &[],
            &["/etc/containers", "$HOME/.config/containers"],
            &["/var/lib/containers", "/run/user"],
        ),
    );

    // Monitoring
    m.insert(
        "prometheus",
        network_server_profile(
            "prometheus",
            &["9090"],
            &["/etc/prometheus"],
            &["/var/lib/prometheus", "/var/log/prometheus"],
        ),
    );

    m.insert(
        "grafana",
        network_server_profile(
            "grafana",
            &["3000"],
            &["/etc/grafana"],
            &["/var/lib/grafana", "/var/log/grafana"],
        ),
    );

    m.insert(
        "node_exporter",
        network_server_profile(
            "node_exporter",
            &["9100"],
            &["/proc", "/sys"],
            &[],
        ),
    );

    // Proxies/Load balancers
    m.insert(
        "haproxy",
        network_server_profile(
            "haproxy",
            &["80", "443", "8404"],
            &["/etc/haproxy", "/etc/ssl/certs"],
            &["/var/lib/haproxy", "/var/log/haproxy", "/run/haproxy"],
        ),
    );

    m.insert(
        "envoy",
        network_server_profile(
            "envoy",
            &["10000", "9901"],
            &["/etc/envoy"],
            &["/var/log/envoy"],
        ),
    );

    // =========================================================================
    // Additional databases
    // =========================================================================
    m.insert(
        "memcached",
        network_server_profile(
            "memcached",
            &["11211"],
            &["/etc/memcached.conf"],
            &["/var/log/memcached", "/run/memcached"],
        ),
    );

    m.insert(
        "etcd",
        network_server_profile(
            "etcd",
            &["2379", "2380"],
            &["/etc/etcd"],
            &["/var/lib/etcd"],
        ),
    );

    m.insert(
        "consul",
        network_server_profile(
            "consul",
            &["8300", "8301", "8302", "8500", "8600"],
            &["/etc/consul.d"],
            &["/var/lib/consul", "/var/log/consul"],
        ),
    );

    m.insert(
        "elasticsearch",
        network_server_profile(
            "elasticsearch",
            &["9200", "9300"],
            &["/etc/elasticsearch"],
            &["/var/lib/elasticsearch", "/var/log/elasticsearch"],
        ),
    );

    m.insert(
        "opensearch",
        network_server_profile(
            "opensearch",
            &["9200", "9300"],
            &["/etc/opensearch"],
            &["/var/lib/opensearch", "/var/log/opensearch"],
        ),
    );

    m.insert(
        "cassandra",
        network_server_profile(
            "cassandra",
            &["7000", "9042"],
            &["/etc/cassandra"],
            &["/var/lib/cassandra", "/var/log/cassandra"],
        ),
    );

    m.insert(
        "couchdb",
        network_server_profile(
            "couchdb",
            &["5984"],
            &["/etc/couchdb"],
            &["/var/lib/couchdb", "/var/log/couchdb"],
        ),
    );

    m.insert(
        "influxdb",
        network_server_profile(
            "influxdb",
            &["8086"],
            &["/etc/influxdb"],
            &["/var/lib/influxdb", "/var/log/influxdb"],
        ),
    );

    m.insert(
        "clickhouse-server",
        network_server_profile(
            "clickhouse-server",
            &["8123", "9000", "9009"],
            &["/etc/clickhouse-server"],
            &["/var/lib/clickhouse", "/var/log/clickhouse-server"],
        ),
    );

    // =========================================================================
    // More web servers
    // =========================================================================
    m.insert(
        "lighttpd",
        network_server_profile(
            "lighttpd",
            &["80", "443"],
            &["/etc/lighttpd"],
            &["/var/log/lighttpd", "/var/cache/lighttpd"],
        ),
    );

    m.insert(
        "traefik",
        network_server_profile(
            "traefik",
            &["80", "443", "8080"],
            &["/etc/traefik"],
            &["/var/log/traefik"],
        ),
    );

    m.insert(
        "openresty",
        network_server_profile(
            "openresty",
            &["80", "443"],
            &["/etc/openresty", "/usr/local/openresty"],
            &["/var/log/openresty", "/var/cache/openresty"],
        ),
    );

    // =========================================================================
    // Mail servers
    // =========================================================================
    m.insert(
        "postfix",
        network_server_profile(
            "postfix",
            &["25", "465", "587"],
            &["/etc/postfix"],
            &["/var/spool/postfix", "/var/log/mail.log"],
        ),
    );

    m.insert(
        "dovecot",
        network_server_profile(
            "dovecot",
            &["143", "993", "110", "995"],
            &["/etc/dovecot"],
            &["/var/lib/dovecot", "/var/log/dovecot"],
        ),
    );

    m.insert(
        "exim4",
        network_server_profile(
            "exim4",
            &["25", "465", "587"],
            &["/etc/exim4"],
            &["/var/spool/exim4", "/var/log/exim4"],
        ),
    );

    m.insert(
        "sendmail",
        network_server_profile(
            "sendmail",
            &["25", "587"],
            &["/etc/mail"],
            &["/var/spool/mqueue", "/var/log/maillog"],
        ),
    );

    // =========================================================================
    // DNS servers
    // =========================================================================
    m.insert(
        "bind9",
        network_server_profile(
            "bind9",
            &["53"],
            &["/etc/bind"],
            &["/var/cache/bind", "/var/log/named"],
        ),
    );

    m.insert(
        "named",
        network_server_profile(
            "named",
            &["53"],
            &["/etc/named.conf", "/var/named"],
            &["/var/log/named"],
        ),
    );

    m.insert(
        "unbound",
        network_server_profile(
            "unbound",
            &["53"],
            &["/etc/unbound"],
            &["/var/lib/unbound", "/var/log/unbound"],
        ),
    );

    m.insert(
        "dnsmasq",
        network_server_profile(
            "dnsmasq",
            &["53", "67"],
            &["/etc/dnsmasq.conf", "/etc/dnsmasq.d"],
            &["/var/lib/misc/dnsmasq.leases", "/var/log/dnsmasq"],
        ),
    );

    m.insert(
        "coredns",
        network_server_profile(
            "coredns",
            &["53"],
            &["/etc/coredns"],
            &["/var/log/coredns"],
        ),
    );

    // =========================================================================
    // FTP servers
    // =========================================================================
    m.insert(
        "vsftpd",
        network_server_profile(
            "vsftpd",
            &["21", "20"],
            &["/etc/vsftpd.conf", "/etc/vsftpd"],
            &["/var/log/vsftpd.log", "/var/ftp"],
        ),
    );

    m.insert(
        "proftpd",
        network_server_profile(
            "proftpd",
            &["21"],
            &["/etc/proftpd"],
            &["/var/log/proftpd"],
        ),
    );

    // =========================================================================
    // VPN
    // =========================================================================
    m.insert(
        "openvpn",
        network_server_profile(
            "openvpn",
            &["1194"],
            &["/etc/openvpn"],
            &["/var/log/openvpn", "/run/openvpn"],
        ),
    );

    m.insert(
        "wireguard",
        daemon_profile(
            "wireguard",
            &["51820"],
            &["/etc/wireguard"],
            &[],
        ),
    );

    m.insert(
        "strongswan",
        network_server_profile(
            "strongswan",
            &["500", "4500"],
            &["/etc/strongswan.d", "/etc/ipsec.conf"],
            &["/var/log/charon.log"],
        ),
    );

    // =========================================================================
    // File servers
    // =========================================================================
    m.insert(
        "nfs-kernel-server",
        network_server_profile(
            "nfs-kernel-server",
            &["2049", "111"],
            &["/etc/exports"],
            &["/var/lib/nfs"],
        ),
    );

    m.insert(
        "samba",
        network_server_profile(
            "samba",
            &["137", "138", "139", "445"],
            &["/etc/samba/smb.conf"],
            &["/var/lib/samba", "/var/log/samba"],
        ),
    );

    m.insert(
        "smbd",
        network_server_profile(
            "smbd",
            &["139", "445"],
            &["/etc/samba/smb.conf"],
            &["/var/lib/samba", "/var/log/samba"],
        ),
    );

    // =========================================================================
    // Time services
    // =========================================================================
    m.insert(
        "chrony",
        daemon_profile(
            "chrony",
            &["123"],
            &["/etc/chrony.conf", "/etc/chrony"],
            &["/var/lib/chrony", "/var/log/chrony"],
        ),
    );

    m.insert(
        "ntp",
        daemon_profile(
            "ntp",
            &["123"],
            &["/etc/ntp.conf"],
            &["/var/lib/ntp", "/var/log/ntpstats"],
        ),
    );

    m.insert(
        "systemd-timesyncd",
        daemon_profile(
            "systemd-timesyncd",
            &[],
            &["/etc/systemd/timesyncd.conf"],
            &["/var/lib/systemd/timesync"],
        ),
    );

    // =========================================================================
    // Logging
    // =========================================================================
    m.insert(
        "rsyslog",
        daemon_profile(
            "rsyslog",
            &["514"],
            &["/etc/rsyslog.conf", "/etc/rsyslog.d"],
            &["/var/log", "/var/spool/rsyslog"],
        ),
    );

    m.insert(
        "syslog-ng",
        daemon_profile(
            "syslog-ng",
            &["514"],
            &["/etc/syslog-ng"],
            &["/var/log"],
        ),
    );

    m.insert(
        "fluentd",
        network_server_profile(
            "fluentd",
            &["24224"],
            &["/etc/fluent"],
            &["/var/log/fluent"],
        ),
    );

    m.insert(
        "logstash",
        network_server_profile(
            "logstash",
            &["5044", "9600"],
            &["/etc/logstash"],
            &["/var/lib/logstash", "/var/log/logstash"],
        ),
    );

    m.insert(
        "filebeat",
        network_client_profile(
            "filebeat",
            &["5044", "9200"],
            &["/etc/filebeat", "/var/log"],
            &["/var/lib/filebeat"],
        ),
    );

    m.insert(
        "vector",
        network_server_profile(
            "vector",
            &["8686"],
            &["/etc/vector"],
            &["/var/lib/vector"],
        ),
    );

    // =========================================================================
    // Security tools
    // =========================================================================
    m.insert(
        "fail2ban",
        daemon_profile(
            "fail2ban",
            &[],
            &["/etc/fail2ban"],
            &["/var/lib/fail2ban", "/var/log/fail2ban.log"],
        ),
    );

    m.insert(
        "ufw",
        cli_profile("ufw", &["/etc/ufw"], &["/var/log/ufw.log"]),
    );

    m.insert(
        "firewalld",
        daemon_profile(
            "firewalld",
            &[],
            &["/etc/firewalld"],
            &["/var/log/firewalld"],
        ),
    );

    m.insert(
        "apparmor",
        daemon_profile(
            "apparmor",
            &[],
            &["/etc/apparmor.d", "/etc/apparmor"],
            &["/var/lib/apparmor"],
        ),
    );

    // =========================================================================
    // Programming languages & runtimes
    // =========================================================================
    m.insert(
        "python",
        cli_profile("python", &["/usr/lib/python*", "$HOME/.local"], &["$HOME/.cache/pip"]),
    );

    m.insert(
        "python3",
        cli_profile("python3", &["/usr/lib/python3*", "$HOME/.local"], &["$HOME/.cache/pip"]),
    );

    m.insert(
        "ruby",
        cli_profile("ruby", &["/usr/lib/ruby", "$HOME/.gem"], &["$HOME/.gem"]),
    );

    m.insert(
        "node",
        cli_profile("node", &["/usr/lib/node_modules", "$HOME/.npm"], &["$HOME/.npm"]),
    );

    m.insert(
        "nodejs",
        cli_profile("nodejs", &["/usr/lib/node_modules", "$HOME/.npm"], &["$HOME/.npm"]),
    );

    m.insert(
        "php",
        cli_profile("php", &["/etc/php", "/usr/lib/php"], &["/var/lib/php"]),
    );

    m.insert(
        "perl",
        cli_profile("perl", &["/usr/share/perl", "/usr/lib/perl"], &[]),
    );

    m.insert(
        "java",
        cli_profile("java", &["/usr/lib/jvm", "/etc/java"], &["$HOME/.java"]),
    );

    m.insert(
        "go",
        cli_profile("go", &["/usr/lib/go", "$HOME/go"], &["$HOME/go"]),
    );

    m.insert(
        "rust",
        cli_profile("rust", &["$HOME/.rustup", "$HOME/.cargo"], &["$HOME/.cargo"]),
    );

    // =========================================================================
    // Package managers
    // =========================================================================
    m.insert(
        "apt",
        cli_profile("apt", &["/etc/apt", "/var/lib/apt"], &["/var/cache/apt", "/var/lib/apt"]),
    );

    m.insert(
        "dpkg",
        cli_profile("dpkg", &["/var/lib/dpkg"], &["/var/lib/dpkg"]),
    );

    m.insert(
        "yum",
        cli_profile("yum", &["/etc/yum.conf", "/etc/yum.repos.d"], &["/var/cache/yum"]),
    );

    m.insert(
        "dnf",
        cli_profile("dnf", &["/etc/dnf", "/etc/yum.repos.d"], &["/var/cache/dnf"]),
    );

    m.insert(
        "pacman",
        cli_profile("pacman", &["/etc/pacman.conf", "/etc/pacman.d"], &["/var/cache/pacman", "/var/lib/pacman"]),
    );

    m.insert(
        "zypper",
        cli_profile("zypper", &["/etc/zypp"], &["/var/cache/zypp"]),
    );

    m.insert(
        "pip",
        network_client_profile("pip", &["443"], &["$HOME/.cache/pip"], &["$HOME/.cache/pip"]),
    );

    m.insert(
        "npm",
        network_client_profile("npm", &["443"], &["$HOME/.npm", "/usr/lib/node_modules"], &["$HOME/.npm"]),
    );

    m.insert(
        "cargo",
        network_client_profile("cargo", &["443"], &["$HOME/.cargo"], &["$HOME/.cargo"]),
    );

    m.insert(
        "gem",
        network_client_profile("gem", &["443"], &["$HOME/.gem"], &["$HOME/.gem"]),
    );

    // =========================================================================
    // Shells
    // =========================================================================
    m.insert(
        "bash",
        cli_profile("bash", &["/etc/bash.bashrc", "$HOME/.bashrc"], &["$HOME/.bash_history"]),
    );

    m.insert(
        "zsh",
        cli_profile("zsh", &["/etc/zsh", "$HOME/.zshrc"], &["$HOME/.zsh_history"]),
    );

    m.insert(
        "fish",
        cli_profile("fish", &["/etc/fish", "$HOME/.config/fish"], &["$HOME/.local/share/fish"]),
    );

    // =========================================================================
    // More CLI tools
    // =========================================================================
    m.insert(
        "tar",
        cli_profile("tar", &["/"], &[]),
    );

    m.insert(
        "gzip",
        cli_profile("gzip", &["/"], &[]),
    );

    m.insert(
        "xz",
        cli_profile("xz", &["/"], &[]),
    );

    m.insert(
        "bzip2",
        cli_profile("bzip2", &["/"], &[]),
    );

    m.insert(
        "unzip",
        cli_profile("unzip", &["/"], &[]),
    );

    m.insert(
        "rsync",
        network_client_profile("rsync", &["22", "873"], &["/"], &["/"]),
    );

    m.insert(
        "openssh-client",
        network_client_profile("openssh-client", &["22"], &["/etc/ssh", "$HOME/.ssh"], &["$HOME/.ssh/known_hosts"]),
    );

    m.insert(
        "ssh",
        network_client_profile("ssh", &["22"], &["/etc/ssh", "$HOME/.ssh"], &["$HOME/.ssh/known_hosts"]),
    );

    m.insert(
        "scp",
        network_client_profile("scp", &["22"], &["/etc/ssh", "$HOME/.ssh"], &["/"]),
    );

    m.insert(
        "less",
        cli_profile("less", &["/"], &[]),
    );

    m.insert(
        "htop",
        cli_profile("htop", &["/proc", "$HOME/.config/htop"], &["$HOME/.config/htop"]),
    );

    m.insert(
        "tmux",
        cli_profile("tmux", &["$HOME/.tmux.conf"], &["/tmp/tmux-*"]),
    );

    m.insert(
        "screen",
        cli_profile("screen", &["$HOME/.screenrc"], &["/var/run/screen"]),
    );

    // =========================================================================
    // Editors
    // =========================================================================
    m.insert(
        "emacs",
        cli_profile("emacs", &["/", "$HOME/.emacs.d"], &["$HOME/.emacs.d"]),
    );

    m.insert(
        "neovim",
        cli_profile("neovim", &["/", "$HOME/.config/nvim"], &["$HOME/.local/share/nvim"]),
    );

    m.insert(
        "nvim",
        cli_profile("nvim", &["/", "$HOME/.config/nvim"], &["$HOME/.local/share/nvim"]),
    );

    // =========================================================================
    // Build tools
    // =========================================================================
    m.insert(
        "ninja",
        cli_profile("ninja", &["/"], &["."]),
    );

    m.insert(
        "meson",
        cli_profile("meson", &["/usr/share/meson"], &["."]),
    );

    m.insert(
        "autoconf",
        cli_profile("autoconf", &["/usr/share/autoconf"], &["."]),
    );

    m.insert(
        "automake",
        cli_profile("automake", &["/usr/share/automake"], &["."]),
    );

    m.insert(
        "llvm",
        cli_profile("llvm", &["/usr/lib/llvm*"], &["/tmp"]),
    );

    m.insert(
        "clang",
        cli_profile("clang", &["/usr/lib/clang", "/usr/include"], &["/tmp"]),
    );

    // =========================================================================
    // Database clients
    // =========================================================================
    m.insert(
        "psql",
        network_client_profile("psql", &["5432"], &["$HOME/.psqlrc", "$HOME/.pgpass"], &["$HOME/.psql_history"]),
    );

    m.insert(
        "mysql-client",
        network_client_profile("mysql-client", &["3306"], &["$HOME/.my.cnf"], &["$HOME/.mysql_history"]),
    );

    m.insert(
        "redis-cli",
        network_client_profile("redis-cli", &["6379"], &[], &[]),
    );

    m.insert(
        "mongosh",
        network_client_profile("mongosh", &["27017"], &["$HOME/.mongoshrc.js"], &["$HOME/.mongosh"]),
    );

    // =========================================================================
    // More monitoring
    // =========================================================================
    m.insert(
        "telegraf",
        network_server_profile(
            "telegraf",
            &["8125"],
            &["/etc/telegraf"],
            &["/var/log/telegraf"],
        ),
    );

    m.insert(
        "zabbix-agent",
        network_server_profile(
            "zabbix-agent",
            &["10050"],
            &["/etc/zabbix"],
            &["/var/log/zabbix"],
        ),
    );

    m.insert(
        "collectd",
        daemon_profile(
            "collectd",
            &["25826"],
            &["/etc/collectd"],
            &["/var/lib/collectd"],
        ),
    );

    m.insert(
        "alertmanager",
        network_server_profile(
            "alertmanager",
            &["9093"],
            &["/etc/alertmanager"],
            &["/var/lib/alertmanager"],
        ),
    );

    // =========================================================================
    // Kubernetes
    // =========================================================================
    m.insert(
        "kubelet",
        daemon_profile(
            "kubelet",
            &["10250", "10255"],
            &["/etc/kubernetes", "/var/lib/kubelet"],
            &["/var/lib/kubelet", "/var/log/pods"],
        ),
    );

    m.insert(
        "kubectl",
        network_client_profile("kubectl", &["6443"], &["$HOME/.kube"], &["$HOME/.kube/cache"]),
    );

    m.insert(
        "kube-proxy",
        daemon_profile(
            "kube-proxy",
            &["10256"],
            &["/etc/kubernetes"],
            &["/var/lib/kube-proxy"],
        ),
    );

    m.insert(
        "kube-apiserver",
        network_server_profile(
            "kube-apiserver",
            &["6443"],
            &["/etc/kubernetes"],
            &["/var/log/kubernetes"],
        ),
    );

    m.insert(
        "helm",
        network_client_profile("helm", &["443"], &["$HOME/.helm", "$HOME/.config/helm"], &["$HOME/.cache/helm"]),
    );

    // =========================================================================
    // Container tools
    // =========================================================================
    m.insert(
        "cri-o",
        daemon_profile(
            "cri-o",
            &[],
            &["/etc/crio"],
            &["/var/lib/containers", "/run/crio"],
        ),
    );

    m.insert(
        "runc",
        daemon_profile(
            "runc",
            &[],
            &[],
            &["/run/runc"],
        ),
    );

    m.insert(
        "buildah",
        cli_profile("buildah", &["$HOME/.config/containers"], &["$HOME/.local/share/containers"]),
    );

    m.insert(
        "skopeo",
        network_client_profile("skopeo", &["443"], &["$HOME/.config/containers"], &[]),
    );

    // =========================================================================
    // CI/CD
    // =========================================================================
    m.insert(
        "jenkins",
        network_server_profile(
            "jenkins",
            &["8080", "50000"],
            &["/etc/jenkins", "/var/lib/jenkins"],
            &["/var/lib/jenkins", "/var/log/jenkins"],
        ),
    );

    m.insert(
        "gitlab-runner",
        network_client_profile(
            "gitlab-runner",
            &["443"],
            &["/etc/gitlab-runner"],
            &["/var/lib/gitlab-runner"],
        ),
    );

    // =========================================================================
    // Networking tools
    // =========================================================================
    m.insert(
        "iperf3",
        network_server_profile(
            "iperf3",
            &["5201"],
            &[],
            &[],
        ),
    );

    m.insert(
        "netcat",
        network_client_profile("netcat", &["*"], &[], &[]),
    );

    m.insert(
        "nmap",
        network_client_profile("nmap", &["*"], &["/usr/share/nmap"], &[]),
    );

    m.insert(
        "tcpdump",
        cli_profile("tcpdump", &["/"], &[]),
    );

    // =========================================================================
    // Vault and secrets
    // =========================================================================
    m.insert(
        "vault",
        network_server_profile(
            "vault",
            &["8200"],
            &["/etc/vault.d"],
            &["/var/lib/vault"],
        ),
    );

    // =========================================================================
    // Service mesh
    // =========================================================================
    m.insert(
        "linkerd-proxy",
        network_server_profile(
            "linkerd-proxy",
            &["4143", "4191"],
            &[],
            &[],
        ),
    );

    m
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lookup_exact() {
        let profile = WellKnownProfiles::lookup("nginx").unwrap();
        assert_eq!(profile.source, InferenceSource::WellKnown);
        assert!(profile.network.listen_ports.contains(&"80".to_string()));
        assert!(profile.network.listen_ports.contains(&"443".to_string()));
    }

    #[test]
    fn test_lookup_versioned() {
        // Should match python for python3.11
        let has_base = WellKnownProfiles::has_profile("nginx");
        assert!(has_base);
    }

    #[test]
    fn test_lookup_not_found() {
        assert!(WellKnownProfiles::lookup("obscure-package-xyz").is_none());
    }

    #[test]
    fn test_database_profiles() {
        let pg = WellKnownProfiles::lookup("postgresql").unwrap();
        assert!(pg.network.listen_ports.contains(&"5432".to_string()));

        let redis = WellKnownProfiles::lookup("redis").unwrap();
        assert!(redis.network.listen_ports.contains(&"6379".to_string()));
    }

    #[test]
    fn test_cli_profiles() {
        let curl = WellKnownProfiles::lookup("curl").unwrap();
        assert!(!curl.network.no_network);
        assert!(curl.network.outbound_ports.contains(&"443".to_string()));

        let grep = WellKnownProfiles::lookup("grep").unwrap();
        assert!(grep.network.no_network);
    }

    #[test]
    fn test_list_known_packages() {
        let packages = WellKnownProfiles::list_known_packages();
        assert!(packages.contains(&"nginx"));
        assert!(packages.contains(&"postgresql"));
        assert!(packages.len() >= 100, "Expected 100+ profiles, got {}", packages.len());
    }

    #[test]
    fn test_new_profiles() {
        // Test some of the newly added profiles
        let etcd = WellKnownProfiles::lookup("etcd").unwrap();
        assert!(etcd.network.listen_ports.contains(&"2379".to_string()));

        let postfix = WellKnownProfiles::lookup("postfix").unwrap();
        assert!(postfix.network.listen_ports.contains(&"25".to_string()));

        let kubectl = WellKnownProfiles::lookup("kubectl").unwrap();
        assert!(kubectl.network.outbound_ports.contains(&"6443".to_string()));

        let bash = WellKnownProfiles::lookup("bash").unwrap();
        assert!(bash.network.no_network);
    }
}

// conary-test/src/engine/mock_server.rs

use std::time::Duration;

use anyhow::{Context, Result, bail};

use crate::config::manifest::MockServerConfig;
use crate::container::backend::{ContainerBackend, ContainerId};

const MOCK_SERVER_SCRIPT_PATH: &str = "/tmp/conary_mock_server.py";

pub fn generate_mock_script(config: &MockServerConfig) -> Result<String> {
    let routes_json =
        serde_json::to_string(&config.routes).context("failed to encode mock routes")?;

    Ok(format!(
        r#"#!/usr/bin/env python3
import json
import pathlib
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = {port}
ROUTES = {{}}


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        self.handle_request()

    def do_POST(self):
        self.handle_request()

    def do_PUT(self):
        self.handle_request()

    def do_DELETE(self):
        self.handle_request()

    def log_message(self, fmt, *args):
        return

    def handle_request(self):
        route = ROUTES.get(self.path)
        if route is None:
            self.send_response(404)
            self.end_headers()
            self.wfile.write(b"not found")
            return

        delay_ms = route.get("delay_ms")
        if delay_ms:
            time.sleep(delay_ms / 1000.0)

        body = route.get("body")
        body_file = route.get("body_file")
        payload = b""
        if body_file:
            payload = pathlib.Path(body_file).read_bytes()
        elif body is not None:
            payload = body.encode("utf-8")

        truncate_at = route.get("truncate_at_bytes")
        if truncate_at is not None:
            payload = payload[:truncate_at]

        self.send_response(route["status"])
        headers = route.get("headers") or {{}}
        for key, value in headers.items():
            self.send_header(key, value)
        if "Content-Length" not in headers:
            self.send_header("Content-Length", str(len(payload)))
        self.end_headers()
        if payload:
            self.wfile.write(payload)


def main():
    route_map = {{}}
    for route in json.loads(r'''{routes_json}'''):
        route_map[route["path"]] = route
    global ROUTES
    ROUTES = route_map

    server = ThreadingHTTPServer(("0.0.0.0", PORT), Handler)
    server.serve_forever()


if __name__ == "__main__":
    main()
"#,
        port = config.port,
        routes_json = routes_json
    ))
}

pub async fn start_mock_server(
    backend: &dyn ContainerBackend,
    container_id: &ContainerId,
    config: &MockServerConfig,
) -> Result<()> {
    let script = generate_mock_script(config)?;
    backend
        .copy_to(container_id, MOCK_SERVER_SCRIPT_PATH, script.as_bytes())
        .await?;
    backend
        .exec_detached(container_id, &["python3", MOCK_SERVER_SCRIPT_PATH])
        .await?;

    let wait_cmd = format!(
        "for _ in $(seq 1 50); do python3 -c \"import socket; s = socket.create_connection(('127.0.0.1', {port}), 0.2); s.close()\" && exit 0; sleep 0.1; done; exit 1",
        port = config.port
    );
    let readiness = backend
        .exec(
            container_id,
            &["sh", "-c", &wait_cmd],
            Duration::from_secs(10),
        )
        .await?;
    if readiness.exit_code != 0 {
        bail!("mock server did not become ready on port {}", config.port);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::manifest::MockRoute;
    use std::collections::HashMap;

    #[test]
    fn test_generate_mock_script() {
        let config = MockServerConfig {
            port: 8888,
            routes: vec![MockRoute {
                path: "/v1/packages/foo.ccs".to_string(),
                status: 429,
                body: None,
                body_file: Some("/tmp/foo.ccs".to_string()),
                headers: Some(HashMap::from([(
                    "Retry-After".to_string(),
                    "1".to_string(),
                )])),
                delay_ms: Some(50),
                truncate_at_bytes: Some(1024),
            }],
        };

        let script = generate_mock_script(&config).unwrap();

        assert!(script.contains("PORT = 8888"));
        assert!(script.contains("ThreadingHTTPServer"));
        assert!(script.contains("\"/v1/packages/foo.ccs\""));
        assert!(script.contains("Retry-After"));
        assert!(script.contains("truncate_at_bytes"));
    }
}

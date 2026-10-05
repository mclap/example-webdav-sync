use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// Fixture for starting/stopping local WebDAV server (wsgidav) in tests
pub struct TestWebDavServer {
    child: Child,
    pub port: u16,
    pub root: PathBuf,
    /// Server temp directory — kept in field so it isn't deleted on Drop
    #[allow(dead_code)]
    _temp_dir: tempfile::TempDir,
}

impl TestWebDavServer {
    /// Start wsgidav with anonymous access
    pub fn start() -> Self {
        Self::start_with_auth("anonymous")
    }

    /// Start wsgidav with Basic Auth
    pub fn start_with_auth(auth: &str) -> Self {
        let temp_dir = tempfile::tempdir().unwrap();
        let root = temp_dir.path().to_path_buf();
        let port = find_free_port().expect("Could not find free port");

        std::fs::create_dir_all(&root).unwrap();

        let site_packages = find_python_site_packages();

        let mut cmd = Command::new("wsgidav");
        cmd.args([
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--root",
            root.to_str().unwrap(),
            "--auth",
            auth,
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

        if let Some(ref path) = site_packages {
            cmd.env("PYTHONPATH", path);
        }

        let child = cmd
            .spawn()
            .expect("Failed to start wsgidav. Make sure it is installed: pip install wsgidav");

        let server = Self {
            child,
            port,
            root,
            _temp_dir: temp_dir,
        };

        wait_for_server(port, Duration::from_secs(10));

        server
    }

    /// Get WebDAV server URL
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}/", self.port)
    }

    /// Create file on server directly (for tests)
    pub fn create_remote_file(&self, rel_path: &str, content: &[u8]) {
        let full_path = self.root.join(rel_path);
        if let Some(parent) = full_path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(full_path, content).unwrap();
    }

    /// Check if file exists on server
    pub fn remote_file_exists(&self, rel_path: &str) -> bool {
        self.root.join(rel_path).exists()
    }
}

impl Drop for TestWebDavServer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Find free TCP port
fn find_free_port() -> Option<u16> {
    use std::net::TcpListener;
    TcpListener::bind("127.0.0.1:0")
        .ok()
        .and_then(|l| l.local_addr().ok())
        .map(|a| a.port())
}

/// Wait for server readiness
fn wait_for_server(port: u16, timeout: Duration) {
    let start = std::time::Instant::now();
    while start.elapsed() < timeout {
        if let Ok(_) = std::net::TcpStream::connect(("127.0.0.1", port)) {
            std::thread::sleep(Duration::from_millis(100));
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("WebDAV server did not start within {:?}", timeout);
}

/// Find Python site-packages path for wsgidav dependencies
fn find_python_site_packages() -> Option<String> {
    let possible_paths = [
        "/tmp/wsgidav-venv/lib/python3.14/site-packages",
        "/usr/lib/python3.14/site-packages",
        "/usr/lib/python3.13/site-packages",
        "/usr/lib/python3.12/site-packages",
        "/usr/lib/python3.11/site-packages",
        "/usr/lib/python3.10/site-packages",
    ];

    for path in &possible_paths {
        let path = std::path::Path::new(path);
        if path.exists() && path.is_dir() {
            if path.join("cheroot").exists() {
                return Some(path.to_string_lossy().to_string());
            }
        }
    }

    if let Ok(output) = Command::new("python3")
        .args([
            "-c",
            "import cheroot; import os; print(os.path.dirname(os.path.dirname(cheroot.__file__)))",
        ])
        .output()
    {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Some(path);
            }
        }
    }

    None
}

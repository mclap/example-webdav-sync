use anyhow::Context;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use reqwest::{Client, Method, StatusCode};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::Duration;
use url::Url;

use crate::config::Config;
use crate::metadata::FileMetadata;

/// WebDAV client with retry support
#[derive(Clone)]
pub struct WebDavClient {
    client: Client,
    base_url: Url,
    auth: (String, String),
    max_retries: u32,
}

impl WebDavClient {
    pub fn new(config: &Config) -> anyhow::Result<Self> {
        let base_url = Url::parse(&config.webdav_url)
            .with_context(|| format!("Invalid WebDAV URL: {}", config.webdav_url))?;

        let base_url = if base_url.path().ends_with('/') {
            base_url
        } else {
            let mut url = base_url.clone();
            url.set_path(&format!("{}/", url.path()));
            url
        };

        let client = Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .connect_timeout(Duration::from_secs(config.timeout_secs))
            .build()
            .context("Failed to create HTTP client")?;

        Ok(Self {
            client,
            base_url,
            auth: (config.username.clone(), config.password.clone()),
            max_retries: config.max_retries,
        })
    }

    /// Execute request with retries
    async fn request(&self, method: Method, path: &str) -> anyhow::Result<reqwest::Response> {
        let url = self.base_url.join(path)?;
        let mut last_err = None;

        for attempt in 0..=self.max_retries {
            if attempt > 0 {
                let delay = Duration::from_millis(200 * 2u64.pow(attempt - 1));
                tokio::time::sleep(delay).await;
                tracing::warn!(
                    "Retry {}/{} for {} {}",
                    attempt,
                    self.max_retries,
                    method,
                    url
                );
            }

            let resp = self
                .client
                .request(method.clone(), url.clone())
                .basic_auth(&self.auth.0, Some(&self.auth.1))
                .send()
                .await;

            match resp {
                Ok(r) => {
                    if r.status().is_success() || r.status() == StatusCode::NOT_FOUND {
                        return Ok(r);
                    }
                    let status = r.status();
                    let body = r.text().await.unwrap_or_default();
                    last_err = Some(anyhow::anyhow!(
                        "HTTP {}: {}",
                        status,
                        body.chars().take(200).collect::<String>()
                    ));
                }
                Err(e) => {
                    last_err = Some(e.into());
                }
            }
        }

        Err(last_err.unwrap_or_else(|| anyhow::anyhow!("All retries exhausted")))
    }

    /// List files and directories recursively via PROPFIND
    pub async fn list_files(&self) -> anyhow::Result<Vec<FileMetadata>> {
        let resp = self
            .request(Method::from_bytes(b"PROPFIND").unwrap(), "")
            .await?;

        if resp.status() == StatusCode::NOT_FOUND {
            self.create_dir("").await?;
            return Ok(Vec::new());
        }

        let body = resp.text().await?;
        parse_propfind_response(&body, &self.base_url)
    }

    /// Download file from WebDAV
    pub async fn download(&self, rel_path: &str) -> anyhow::Result<Bytes> {
        let resp = self.request(Method::GET, rel_path).await?;
        let bytes = resp
            .bytes()
            .await
            .with_context(|| format!("Error reading file: {}", rel_path))?;
        Ok(bytes)
    }

    /// Upload file to WebDAV
    pub async fn upload(&self, rel_path: &str, data: Bytes) -> anyhow::Result<()> {
        if let Some(parent) = Path::new(rel_path).parent() {
            if !parent.as_os_str().is_empty() {
                self.create_dir(&parent.to_string_lossy()).await?;
            }
        }

        let resp = self
            .client
            .put(self.base_url.join(rel_path)?)
            .basic_auth(&self.auth.0, Some(&self.auth.1))
            .body(data)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("Upload error {}: HTTP {}: {}", rel_path, status, body);
        }

        Ok(())
    }

    /// Delete file on WebDAV
    pub async fn delete(&self, rel_path: &str) -> anyhow::Result<()> {
        let resp = self.request(Method::DELETE, rel_path).await?;
        if resp.status().is_success() || resp.status() == StatusCode::NOT_FOUND {
            Ok(())
        } else {
            anyhow::bail!("Delete error {}: HTTP {}", rel_path, resp.status())
        }
    }

    /// Create directory on WebDAV (MKCOL)
    pub async fn create_dir(&self, rel_path: &str) -> anyhow::Result<()> {
        let path = if rel_path.is_empty() {
            String::new()
        } else {
            format!("{}/", rel_path)
        };

        let resp = self
            .client
            .request(
                Method::from_bytes(b"MKCOL").unwrap(),
                self.base_url.join(&path)?,
            )
            .basic_auth(&self.auth.0, Some(&self.auth.1))
            .send()
            .await?;

        if resp.status().is_success()
            || resp.status() == StatusCode::METHOD_NOT_ALLOWED
            || resp.status() == StatusCode::CONFLICT
        {
            Ok(())
        } else {
            anyhow::bail!(
                "Create directory error {}: HTTP {}",
                rel_path,
                resp.status()
            )
        }
    }

    /// Move/rename file on WebDAV (MOVE)
    #[allow(dead_code)]
    pub async fn rename(&self, from: &str, to: &str) -> anyhow::Result<()> {
        let destination = self.base_url.join(to)?.to_string();

        let resp = self
            .client
            .request(
                Method::from_bytes(b"MOVE").unwrap(),
                self.base_url.join(from)?,
            )
            .basic_auth(&self.auth.0, Some(&self.auth.1))
            .header("Destination", destination)
            .header("Overwrite", "T")
            .send()
            .await?;

        if resp.status().is_success() {
            Ok(())
        } else {
            anyhow::bail!("Move error {} -> {}: HTTP {}", from, to, resp.status())
        }
    }

    /// Compute SHA-256 hash of content
    #[allow(dead_code)]
    pub fn compute_hash(data: &[u8]) -> String {
        let mut hasher = Sha256::new();
        hasher.update(data);
        format!("{:x}", hasher.finalize())
    }
}

/// Parse PROPFIND response (WebDAV multistatus XML)
fn parse_propfind_response(xml: &str, base_url: &Url) -> anyhow::Result<Vec<FileMetadata>> {
    use quick_xml::events::Event;
    use quick_xml::Reader;

    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut files = Vec::new();
    let mut current_href: Option<String> = None;
    let mut current_size: Option<u64> = None;
    let mut current_modified: Option<DateTime<Utc>> = None;
    let mut current_etag: Option<String> = None;
    let mut current_is_dir = false;
    let mut in_href = false;
    let mut in_getcontentlength = false;
    let mut in_getlastmodified = false;
    let mut in_getetag = false;
    let mut buf = Vec::new();

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                let local_name = name.split(':').next_back().unwrap_or(&name);
                match local_name {
                    "href" => in_href = true,
                    "getcontentlength" => in_getcontentlength = true,
                    "getlastmodified" => in_getlastmodified = true,
                    "getetag" => in_getetag = true,
                    _ => {}
                }
            }
            Ok(Event::End(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                let local_name = name.split(':').next_back().unwrap_or(&name);
                match local_name {
                    "response" => {
                        if let Some(href) = current_href.take() {
                            if let Ok(decoded) =
                                percent_encoding::percent_decode_str(&href).decode_utf8()
                            {
                                let path = decoded.as_ref();
                                {
                                    if let Ok(rel) = strip_base(path, base_url) {
                                        let is_dir = rel.ends_with('/') || current_is_dir;
                                        let clean_rel = rel.trim_end_matches('/').to_string();
                                        if !clean_rel.is_empty() {
                                            files.push(FileMetadata {
                                                rel_path: clean_rel,
                                                size: current_size.take().unwrap_or(0),
                                                modified: current_modified
                                                    .take()
                                                    .unwrap_or_else(|| Utc::now()),
                                                etag: current_etag.take(),
                                                content_hash: None,
                                                is_dir,
                                            });
                                        }
                                    }
                                }
                            }
                        }
                        current_is_dir = false;
                    }
                    "href" => in_href = false,
                    "getcontentlength" => in_getcontentlength = false,
                    "getlastmodified" => in_getlastmodified = false,
                    "getetag" => in_getetag = false,
                    _ => {}
                }
            }
            Ok(Event::Empty(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                let local_name = name.split(':').next_back().unwrap_or(&name);
                if local_name == "collection" {
                    current_is_dir = true;
                }
            }
            Ok(Event::Text(e)) => {
                let text = e.unescape().unwrap_or_default().to_string();
                if in_href {
                    current_href = Some(text);
                } else if in_getcontentlength {
                    current_size = text.trim().parse().ok();
                } else if in_getlastmodified {
                    current_modified = parse_http_date(&text);
                } else if in_getetag {
                    current_etag = Some(text.trim().to_string());
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(anyhow::anyhow!(
                    "XML parse error at position {}: {}",
                    reader.buffer_position(),
                    e
                ));
            }
            _ => {}
        }
        buf.clear();
    }

    Ok(files)
}

/// Strip base URL prefix from path
fn strip_base(path: &str, base_url: &Url) -> anyhow::Result<String> {
    let base_path = base_url.path();
    let path = path.trim_start_matches(base_path);
    Ok(path.trim_start_matches('/').to_string())
}

/// Parse HTTP date (RFC 1123)
fn parse_http_date(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc2822(s)
        .ok()
        .map(|dt| dt.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Datelike;

    #[test]
    fn test_parse_http_date_valid() {
        let result = parse_http_date("Tue, 15 Nov 1994 08:12:31 GMT");
        assert!(result.is_some());
        let dt = result.unwrap();
        assert_eq!(dt.year(), 1994);
        assert_eq!(dt.month(), 11);
        assert_eq!(dt.day(), 15);
    }

    #[test]
    fn test_parse_http_date_invalid() {
        assert!(parse_http_date("not a date").is_none());
        assert!(parse_http_date("").is_none());
    }

    #[test]
    fn test_parse_propfind_response_basic() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<D:multistatus xmlns:D="DAV:">
  <D:response>
    <D:href>/dav/sync/file1.txt</D:href>
    <D:propstat>
      <D:prop>
        <D:getcontentlength>1024</D:getcontentlength>
        <D:getlastmodified>Tue, 15 Nov 1994 08:12:31 GMT</D:getlastmodified>
        <D:getetag>"abc123"</D:getetag>
      </D:prop>
      <D:status>HTTP/1.1 200 OK</D:status>
    </D:propstat>
  </D:response>
  <D:response>
    <D:href>/dav/sync/file2.txt</D:href>
    <D:propstat>
      <D:prop>
        <D:getcontentlength>2048</D:getcontentlength>
        <D:getlastmodified>Wed, 16 Nov 1994 09:13:32 GMT</D:getlastmodified>
      </D:prop>
      <D:status>HTTP/1.1 200 OK</D:status>
    </D:propstat>
  </D:response>
</D:multistatus>"#;

        let base_url = Url::parse("https://example.com/dav/sync/").unwrap();
        let files = parse_propfind_response(xml, &base_url).unwrap();

        assert_eq!(files.len(), 2);
        assert_eq!(files[0].rel_path, "file1.txt");
        assert_eq!(files[0].size, 1024);
        assert!(!files[0].is_dir);
        assert_eq!(files[0].etag.as_deref(), Some("\"abc123\""));
        assert_eq!(files[1].rel_path, "file2.txt");
        assert_eq!(files[1].size, 2048);
    }

    #[test]
    fn test_parse_propfind_response_nested_dirs() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<D:multistatus xmlns:D="DAV:">
  <D:response>
    <D:href>/dav/sync/</D:href>
    <D:propstat>
      <D:prop>
        <D:resourcetype><D:collection/></D:resourcetype>
        <D:getcontentlength>0</D:getcontentlength>
        <D:getlastmodified>Tue, 15 Nov 1994 08:12:31 GMT</D:getlastmodified>
      </D:prop>
      <D:status>HTTP/1.1 200 OK</D:status>
    </D:propstat>
  </D:response>
  <D:response>
    <D:href>/dav/sync/subdir/</D:href>
    <D:propstat>
      <D:prop>
        <D:resourcetype><D:collection/></D:resourcetype>
        <D:getcontentlength>0</D:getcontentlength>
        <D:getlastmodified>Tue, 15 Nov 1994 08:12:31 GMT</D:getlastmodified>
      </D:prop>
      <D:status>HTTP/1.1 200 OK</D:status>
    </D:propstat>
  </D:response>
  <D:response>
    <D:href>/dav/sync/subdir/file.txt</D:href>
    <D:propstat>
      <D:prop>
        <D:resourcetype/>
        <D:getcontentlength>500</D:getcontentlength>
        <D:getlastmodified>Tue, 15 Nov 1994 08:12:31 GMT</D:getlastmodified>
      </D:prop>
      <D:status>HTTP/1.1 200 OK</D:status>
    </D:propstat>
  </D:response>
</D:multistatus>"#;

        let base_url = Url::parse("https://example.com/dav/sync/").unwrap();
        let files = parse_propfind_response(xml, &base_url).unwrap();

        assert_eq!(files.len(), 2);
        assert_eq!(files[0].rel_path, "subdir");
        assert!(files[0].is_dir);
        assert_eq!(files[1].rel_path, "subdir/file.txt");
        assert!(!files[1].is_dir);
        assert_eq!(files[1].size, 500);
    }

    #[test]
    fn test_parse_propfind_response_empty() {
        let xml = r#"<?xml version="1.0" encoding="utf-8"?>
<D:multistatus xmlns:D="DAV:">
</D:multistatus>"#;

        let base_url = Url::parse("https://example.com/dav/sync/").unwrap();
        let files = parse_propfind_response(xml, &base_url).unwrap();
        assert_eq!(files.len(), 0);
    }

    #[test]
    fn test_compute_hash() {
        let data = b"hello world";
        let hash = WebDavClient::compute_hash(data);
        assert_eq!(hash.len(), 64);
        assert_eq!(
            hash,
            "b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9"
        );
    }

    #[test]
    fn test_compute_hash_empty() {
        let hash = WebDavClient::compute_hash(b"");
        assert_eq!(
            hash,
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn test_strip_base() {
        let base = Url::parse("https://example.com/dav/sync/").unwrap();
        assert_eq!(strip_base("/dav/sync/file.txt", &base).unwrap(), "file.txt");
        assert_eq!(
            strip_base("/dav/sync/subdir/file.txt", &base).unwrap(),
            "subdir/file.txt"
        );
        assert_eq!(strip_base("/dav/sync/", &base).unwrap(), "");
    }
}

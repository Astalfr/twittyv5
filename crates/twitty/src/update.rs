//! Updates from the project's GitHub Releases: finding the latest release,
//! downloading its installer and starting it.
//!
//! What to offer is decided in [`crate::state::update`]. The app reaches this
//! module through [`UpdateServices`], which `main.rs` provides only to an
//! installed copy (tests and `cargo run` builds get none).

use std::{
    fmt, fs,
    future::Future,
    io::Write as _,
    path::{Path, PathBuf},
    pin::Pin,
    sync::Arc,
    time::Duration,
};

use serde::Deserialize;
use sha2::Digest as _;

use crate::state::update::{Asset, Release, ReleaseError, Sha256};

type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Why checking for, downloading or starting an update failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UpdateError {
    #[error("could not reach GitHub: {0}")]
    Network(String),
    #[error("GitHub answered {0}")]
    Status(u16),
    #[error("GitHub's answer is unreadable: {0}")]
    Invalid(String),
    #[error(transparent)]
    Release(#[from] ReleaseError),
    #[error("could not save the installer: {0}")]
    Io(String),
    #[error("the downloaded installer is corrupt (SHA-256 {actual}, expected {expected})")]
    Checksum { expected: String, actual: String },
    #[error("could not start the installer: {0}")]
    Launch(String),
}

impl From<reqwest::Error> for UpdateError {
    fn from(e: reqwest::Error) -> Self {
        // Without the URL: the error is shown in a toast.
        Self::Network(e.without_url().to_string())
    }
}

impl From<std::io::Error> for UpdateError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

/// Where releases come from.
pub trait ReleaseSource: Send + Sync {
    /// The latest published release; `None` when there is none yet.
    fn latest(&self) -> BoxFuture<'_, Result<Option<Release>, UpdateError>>;
    /// Downloads `release`'s installer, checks its digest and returns its path.
    fn download<'a>(&'a self, release: &'a Release) -> BoxFuture<'a, Result<PathBuf, UpdateError>>;
}

/// Starts a downloaded installer. The app quits right after.
pub type Launcher = Arc<dyn Fn(&Path) -> Result<(), UpdateError> + Send + Sync>;

/// What the app needs to update itself.
#[derive(Clone)]
pub struct UpdateServices {
    pub source: Arc<dyn ReleaseSource>,
    pub launch: Launcher,
}

impl fmt::Debug for UpdateServices {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("UpdateServices").finish_non_exhaustive()
    }
}

/// Whether `exe` was installed by Twitty's installer, which puts its
/// uninstaller next to it. Only such a copy can update itself.
pub fn installed_by_setup(exe: &Path) -> bool {
    exe.parent().is_some_and(|dir| dir.join("unins000.exe").is_file())
}

/// Runs the installer without its wizard. It closes Twitty if it is still
/// running, keeps the install folder and mode, and starts Twitty again
/// (`/relaunch=1`, see `installer/twitty.iss`).
pub fn launch_installer(path: &Path) -> Result<(), UpdateError> {
    std::process::Command::new(path)
        .args(["/SILENT", "/SUPPRESSMSGBOXES", "/NORESTART", "/CLOSEAPPLICATIONS", "/relaunch=1"])
        .spawn()
        .map(drop)
        .map_err(|e| UpdateError::Launch(e.to_string()))
}

/// The releases of a GitHub repository, through the public REST API.
#[derive(Debug, Clone)]
pub struct GitHubReleases {
    http: reqwest::Client,
    api: String,
    repo: String,
    downloads: PathBuf,
}

/// GitHub's REST API.
pub const GITHUB_API: &str = "https://api.github.com";
/// The repository Twitty's releases are published in.
pub const REPOSITORY: &str = "Astalfr/twittyv5";
/// How long GitHub may stay silent (connecting, or between two reads) before
/// a check or download fails. A slow download that keeps going isn't cut.
pub const STALL_TIMEOUT: Duration = Duration::from_secs(30);

/// The part of GitHub's release object we read.
#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GitHubAsset>,
}

#[derive(Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    digest: Option<String>,
}

impl GitHubReleases {
    /// Releases of `repo` (`owner/name`) from the API at `api`; installers are
    /// downloaded into `downloads`. A request fails after `stall` without data.
    pub fn new(
        api: &str,
        repo: &str,
        downloads: PathBuf,
        stall: Duration,
    ) -> Result<Self, UpdateError> {
        let http = reqwest::Client::builder()
            // GitHub refuses requests without a user agent.
            .user_agent(concat!("twitty/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(stall)
            .read_timeout(stall)
            .build()?;
        Ok(Self {
            http,
            api: api.trim_end_matches('/').to_owned(),
            repo: repo.to_owned(),
            downloads,
        })
    }

    async fn fetch_latest(&self) -> Result<Option<Release>, UpdateError> {
        let url = format!("{}/repos/{}/releases/latest", self.api, self.repo);
        let response = self
            .http
            .get(url)
            .header("accept", "application/vnd.github+json")
            .header("x-github-api-version", "2022-11-28")
            .send()
            .await?;
        // No release published yet.
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(UpdateError::Status(response.status().as_u16()));
        }
        let body = response.bytes().await?;
        let release: GitHubRelease =
            serde_json::from_slice(&body).map_err(|e| UpdateError::Invalid(e.to_string()))?;
        if release.draft || release.prerelease {
            return Ok(None);
        }
        let assets = release
            .assets
            .into_iter()
            .map(|a| Asset {
                name: a.name,
                url: a.browser_download_url,
                sha256: a.digest.as_deref().and_then(Sha256::from_github),
            })
            .collect();
        Ok(Some(Release::from_assets(&release.tag_name, &release.html_url, assets)?))
    }

    async fn fetch_installer(&self, release: &Release) -> Result<PathBuf, UpdateError> {
        fs::create_dir_all(&self.downloads)?;
        let path = self.downloads.join(&release.installer.name);
        let partial = path.with_extension("exe.part");
        let result = self.write_installer(release, &partial).await;
        if let Err(e) = result {
            // Never leave a half-written or corrupt installer behind.
            fs::remove_file(&partial).ok();
            return Err(e);
        }
        fs::rename(&partial, &path)?;
        Ok(path)
    }

    /// Streams the installer into `partial`, hashing as it goes.
    async fn write_installer(&self, release: &Release, partial: &Path) -> Result<(), UpdateError> {
        let mut response = self.http.get(&release.installer.url).send().await?;
        if !response.status().is_success() {
            return Err(UpdateError::Status(response.status().as_u16()));
        }
        let mut file = fs::File::create(partial)?;
        let mut hasher = sha2::Sha256::new();
        while let Some(chunk) = response.chunk().await? {
            hasher.update(&chunk);
            file.write_all(&chunk)?;
        }
        file.sync_all()?;
        let actual = hex(&hasher.finalize());
        if actual != release.sha256.as_hex() {
            return Err(UpdateError::Checksum {
                expected: release.sha256.as_hex().to_owned(),
                actual,
            });
        }
        Ok(())
    }
}

impl ReleaseSource for GitHubReleases {
    fn latest(&self) -> BoxFuture<'_, Result<Option<Release>, UpdateError>> {
        Box::pin(self.fetch_latest())
    }

    fn download<'a>(&'a self, release: &'a Release) -> BoxFuture<'a, Result<PathBuf, UpdateError>> {
        Box::pin(self.fetch_installer(release))
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::update::{Version, installer_name};
    use pretty_assertions::assert_eq;
    use wiremock::{
        Mock, MockServer, ResponseTemplate,
        matchers::{header, method, path},
    };

    const REPO: &str = "owner/twitty";
    const INSTALLER: &[u8] = b"pretend this is an installer";

    fn digest_of(bytes: &[u8]) -> String {
        hex(&sha2::Sha256::digest(bytes))
    }

    fn release_json(server: &MockServer, tag: &str, digest: Option<&str>) -> serde_json::Value {
        let version = Version::parse(tag).unwrap();
        let name = installer_name(version);
        serde_json::json!({
            "tag_name": tag,
            "html_url": format!("https://github.com/{REPO}/releases/tag/{tag}"),
            "draft": false,
            "prerelease": false,
            "assets": [
                {
                    "name": "twitty.exe",
                    "browser_download_url": format!("{}/download/twitty.exe", server.uri()),
                    "digest": digest.map(|d| format!("sha256:{d}")),
                },
                {
                    "name": name,
                    "browser_download_url": format!("{}/download/{name}", server.uri()),
                    "digest": digest.map(|d| format!("sha256:{d}")),
                },
            ],
        })
    }

    async fn serve_latest(server: &MockServer, response: ResponseTemplate) {
        Mock::given(method("GET"))
            .and(path(format!("/repos/{REPO}/releases/latest")))
            .and(header("accept", "application/vnd.github+json"))
            .respond_with(response)
            .mount(server)
            .await;
    }

    fn source(server: &MockServer, dir: &Path) -> GitHubReleases {
        GitHubReleases::new(&server.uri(), REPO, dir.to_path_buf(), STALL_TIMEOUT).unwrap()
    }

    #[tokio::test]
    async fn the_latest_release_and_its_installer_are_read() {
        let server = MockServer::start().await;
        let digest = digest_of(INSTALLER);
        let json = release_json(&server, "v0.2.0", Some(&digest));
        serve_latest(&server, ResponseTemplate::new(200).set_body_json(json)).await;
        let dir = tempfile::tempdir().unwrap();

        let release = source(&server, dir.path()).latest().await.unwrap().unwrap();
        assert_eq!(release.version, Version::parse("0.2.0").unwrap());
        assert_eq!(release.installer.name, "twitty-setup-0.2.0-x64.exe");
        assert_eq!(
            release.installer.url,
            format!("{}/download/{}", server.uri(), release.installer.name)
        );
        assert_eq!(release.sha256.as_hex(), digest);
        assert_eq!(release.page, format!("https://github.com/{REPO}/releases/tag/v0.2.0"));
    }

    #[tokio::test]
    async fn no_release_yet_is_not_an_error() {
        let server = MockServer::start().await;
        serve_latest(&server, ResponseTemplate::new(404)).await;
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(source(&server, dir.path()).latest().await, Ok(None));
    }

    #[tokio::test]
    async fn pre_releases_are_ignored() {
        let server = MockServer::start().await;
        let mut json = release_json(&server, "v0.2.0", Some(&digest_of(INSTALLER)));
        json["prerelease"] = true.into();
        serve_latest(&server, ResponseTemplate::new(200).set_body_json(json)).await;
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(source(&server, dir.path()).latest().await, Ok(None));
    }

    #[tokio::test]
    async fn rate_limits_and_bad_releases_are_errors() {
        let server = MockServer::start().await;
        serve_latest(&server, ResponseTemplate::new(403)).await;
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(source(&server, dir.path()).latest().await, Err(UpdateError::Status(403)));

        let server = MockServer::start().await;
        let json = release_json(&server, "v0.2.0", None);
        serve_latest(&server, ResponseTemplate::new(200).set_body_json(json)).await;
        assert!(matches!(
            source(&server, dir.path()).latest().await,
            Err(UpdateError::Release(ReleaseError::NoDigest(_)))
        ));

        let server = MockServer::start().await;
        serve_latest(&server, ResponseTemplate::new(200).set_body_string("<html>")).await;
        assert!(matches!(source(&server, dir.path()).latest().await, Err(UpdateError::Invalid(_))));
    }

    async fn release_serving(server: &MockServer, digest: &str) -> Release {
        let json = release_json(server, "v0.2.0", Some(digest));
        serve_latest(server, ResponseTemplate::new(200).set_body_json(json)).await;
        Mock::given(method("GET"))
            .and(path("/download/twitty-setup-0.2.0-x64.exe"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(INSTALLER))
            .mount(server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        source(server, dir.path()).latest().await.unwrap().unwrap()
    }

    #[tokio::test]
    async fn the_installer_is_downloaded_and_checked() {
        let server = MockServer::start().await;
        let release = release_serving(&server, &digest_of(INSTALLER)).await;
        let dir = tempfile::tempdir().unwrap();

        let path = source(&server, dir.path()).download(&release).await.unwrap();
        assert_eq!(path, dir.path().join("twitty-setup-0.2.0-x64.exe"));
        assert_eq!(fs::read(&path).unwrap(), INSTALLER);
        let files: Vec<_> = fs::read_dir(dir.path()).unwrap().map(|e| e.unwrap().path()).collect();
        assert_eq!(files, vec![path], "no partial file left");
    }

    #[tokio::test]
    async fn a_corrupt_download_is_refused_and_removed() {
        let server = MockServer::start().await;
        let release = release_serving(&server, &digest_of(b"another file")).await;
        let dir = tempfile::tempdir().unwrap();

        let result = source(&server, dir.path()).download(&release).await;
        assert!(
            matches!(&result, Err(UpdateError::Checksum { actual, .. }) if *actual == digest_of(INSTALLER)),
            "{result:?}"
        );
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0, "nothing left to run");
    }

    #[tokio::test]
    async fn a_stalled_github_is_an_error_not_a_hang() {
        let server = MockServer::start().await;
        let release = release_serving(&server, &digest_of(INSTALLER)).await;
        let stalled = MockServer::start().await;
        let delay = std::time::Duration::from_secs(5);
        serve_latest(&stalled, ResponseTemplate::new(404).set_delay(delay)).await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(INSTALLER).set_delay(delay))
            .mount(&stalled)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let quick = Duration::from_millis(200);
        let source =
            GitHubReleases::new(&stalled.uri(), REPO, dir.path().to_path_buf(), quick).unwrap();

        assert!(matches!(source.latest().await, Err(UpdateError::Network(_))));
        let release = Release {
            installer: Asset {
                url: format!("{}/download/{}", stalled.uri(), release.installer.name),
                ..release.installer
            },
            ..release
        };
        assert!(matches!(source.download(&release).await, Err(UpdateError::Network(_))));
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0, "no partial file left");
    }

    #[test]
    fn only_a_copy_installed_by_setup_updates_itself() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("twitty.exe");
        assert!(!installed_by_setup(&exe), "e.g. target/release");
        fs::write(dir.path().join("unins000.exe"), b"").unwrap();
        assert!(installed_by_setup(&exe));
    }
}

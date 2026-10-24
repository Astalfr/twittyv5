//! Updates from GitHub Releases: version numbers, which release asset is the
//! installer, and what the update banner shows.
//!
//! Fetching and installing live in `crate::update`; this module only decides.

use std::fmt;

/// A release version, `major.minor.patch`. Pre-release and build suffixes
/// aren't supported: GitHub's "latest release" never points at a pre-release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
}

impl Version {
    /// Parses `1.2.3` or a tag like `v1.2.3`.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let text = text.strip_prefix('v').unwrap_or(text);
        let mut parts = text.split('.').map(|p| {
            // `u64::from_str` accepts a leading `+`; a version doesn't.
            if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
                None
            } else {
                p.parse::<u64>().ok()
            }
        });
        let version = Self { major: parts.next()??, minor: parts.next()??, patch: parts.next()?? };
        parts.next().is_none().then_some(version)
    }

    /// This build's version.
    pub fn current() -> Self {
        // Cargo guarantees `CARGO_PKG_VERSION` is semver; a pre-release build
        // (never published) would fall back to 0.0.0 and see every release.
        Self::parse(env!("CARGO_PKG_VERSION")).unwrap_or(Self { major: 0, minor: 0, patch: 0 })
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// A SHA-256 digest, as lowercase hex.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sha256(String);

impl Sha256 {
    /// Parses GitHub's asset `digest` (`sha256:<64 hex>`).
    pub fn from_github(digest: &str) -> Option<Self> {
        Self::from_hex(digest.strip_prefix("sha256:")?)
    }

    /// Parses 64 hex digits, in any case.
    pub fn from_hex(hex: &str) -> Option<Self> {
        (hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| Self(hex.to_ascii_lowercase()))
    }

    pub fn as_hex(&self) -> &str {
        &self.0
    }
}

/// One file attached to a release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    /// GitHub's digest of the file, when it reports one.
    pub sha256: Option<Sha256>,
}

/// A published release that can be installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: Version,
    /// The release's page on GitHub (the notes).
    pub page: String,
    /// The installer to download.
    pub installer: Asset,
    /// The installer's digest, which the download must match.
    pub sha256: Sha256,
}

/// Why a published release can't be offered.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ReleaseError {
    #[error("release tag {0:?} is not a version")]
    BadTag(String),
    #[error("release {0} has no {1}")]
    NoInstaller(Version, String),
    #[error("release {0}'s installer has no SHA-256 digest")]
    NoDigest(Version),
}

/// The installer's file name in release `version`, as CI publishes it.
pub fn installer_name(version: Version) -> String {
    format!("twitty-setup-{version}-x64.exe")
}

impl Release {
    /// The installable release behind a GitHub release: its tag must be a
    /// version, and it must carry that version's installer with a digest.
    pub fn from_assets(tag: &str, page: &str, assets: Vec<Asset>) -> Result<Self, ReleaseError> {
        let version = Version::parse(tag).ok_or_else(|| ReleaseError::BadTag(tag.to_owned()))?;
        let name = installer_name(version);
        let installer = assets
            .into_iter()
            .find(|a| a.name == name)
            .ok_or(ReleaseError::NoInstaller(version, name))?;
        let sha256 = installer.sha256.clone().ok_or(ReleaseError::NoDigest(version))?;
        Ok(Self { version, page: page.to_owned(), installer, sha256 })
    }
}

/// What the update banner shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Nothing to offer (not checked yet, up to date, or dismissed).
    Idle,
    /// A newer release can be installed.
    Available(Release),
    /// Its installer is being downloaded.
    Downloading(Release),
    /// The download or the installer's launch failed.
    Failed(Release, String),
}

/// The update offer, for one session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Updates {
    current: Version,
    status: Status,
    /// A version the user said "Later" to; it isn't offered again this session.
    dismissed: Option<Version>,
}

impl Updates {
    pub fn new(current: Version) -> Self {
        Self { current, status: Status::Idle, dismissed: None }
    }

    pub fn current(&self) -> Version {
        self.current
    }

    pub fn status(&self) -> &Status {
        &self.status
    }

    /// Takes in the latest release; returns whether it is newer than this
    /// build. A newer release is offered unless it was dismissed, and never
    /// interrupts a download.
    pub fn found(&mut self, latest: Option<Release>, now_offering: bool) -> bool {
        let Some(release) = latest.filter(|r| r.version > self.current) else {
            return false;
        };
        let busy = matches!(self.status, Status::Downloading(_));
        let dismissed = !now_offering && self.dismissed.is_some_and(|v| v >= release.version);
        if !busy && !dismissed {
            self.status = Status::Available(release);
        }
        true
    }

    /// "Later": hides the banner until a newer release, or a manual check.
    pub fn dismiss(&mut self) {
        if let Status::Available(r) | Status::Failed(r, _) = &self.status {
            self.dismissed = Some(r.version);
            self.status = Status::Idle;
        }
    }

    /// "Install": the release to download, if one is on offer.
    pub fn start(&mut self) -> Option<Release> {
        match &self.status {
            Status::Available(r) | Status::Failed(r, _) => {
                let release = r.clone();
                self.status = Status::Downloading(release.clone());
                Some(release)
            }
            Status::Idle | Status::Downloading(_) => None,
        }
    }

    /// The download or launch failed; "Install" can be tried again.
    pub fn failed(&mut self, reason: impl Into<String>) {
        if let Status::Downloading(r) = &self.status {
            self.status = Status::Failed(r.clone(), reason.into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use proptest::prelude::*;

    fn v(major: u64, minor: u64, patch: u64) -> Version {
        Version { major, minor, patch }
    }

    const HEX: &str = "3750b2e93f37e0c692657da574d7019a101c0084da05a790c83fd335bad973e4";

    fn asset(name: &str, digest: Option<&str>) -> Asset {
        Asset {
            name: name.to_owned(),
            url: format!("https://example.test/{name}"),
            sha256: digest.and_then(Sha256::from_hex),
        }
    }

    fn release(version: Version) -> Release {
        Release::from_assets(
            &format!("v{version}"),
            "https://example.test/notes",
            vec![asset(&installer_name(version), Some(HEX))],
        )
        .unwrap()
    }

    #[test]
    fn versions_parse_with_or_without_v() {
        assert_eq!(Version::parse("0.1.0"), Some(v(0, 1, 0)));
        assert_eq!(Version::parse("v12.3.45"), Some(v(12, 3, 45)));
        for bad in ["", "v", "1.2", "1.2.3.4", "1.2.x", "1.2.3-beta", "+1.2.3", "1..3", "vv1.2.3"] {
            assert_eq!(Version::parse(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn versions_compare_numerically() {
        assert!(v(0, 10, 0) > v(0, 9, 9));
        assert!(v(1, 0, 0) > v(0, 99, 99));
        assert!(v(0, 1, 1) > v(0, 1, 0));
        assert_eq!(v(1, 2, 3).to_string(), "1.2.3");
    }

    #[test]
    fn current_is_this_crates_version() {
        assert_eq!(Version::current().to_string(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn digests_come_from_githubs_field() {
        let upper = HEX.to_ascii_uppercase();
        assert_eq!(Sha256::from_github(&format!("sha256:{upper}")).unwrap().as_hex(), HEX);
        assert_eq!(Sha256::from_github(HEX), None, "no algorithm prefix");
        assert_eq!(Sha256::from_github(&format!("sha512:{HEX}")), None);
        assert_eq!(Sha256::from_github("sha256:abc"), None);
    }

    #[test]
    fn the_installer_is_picked_by_its_exact_name() {
        let assets = vec![
            asset("twitty.exe", Some(HEX)),
            asset("twitty-setup-0.2.0-arm64.exe", Some(HEX)),
            asset("twitty-setup-0.2.0-x64.exe", Some(HEX)),
        ];
        let r = Release::from_assets("v0.2.0", "https://example.test/notes", assets).unwrap();
        assert_eq!(r.version, v(0, 2, 0));
        assert_eq!(r.installer.name, "twitty-setup-0.2.0-x64.exe");
        assert_eq!(r.sha256.as_hex(), HEX);
    }

    #[test]
    fn releases_without_a_usable_installer_are_refused() {
        let page = "https://example.test/notes";
        assert_eq!(
            Release::from_assets("nightly", page, vec![]),
            Err(ReleaseError::BadTag("nightly".into()))
        );
        // An installer of another version (a mislabelled release) doesn't count.
        let other = vec![asset("twitty-setup-0.1.0-x64.exe", Some(HEX))];
        assert!(matches!(
            Release::from_assets("v0.2.0", page, other),
            Err(ReleaseError::NoInstaller(..))
        ));
        let no_digest = vec![asset("twitty-setup-0.2.0-x64.exe", None)];
        assert_eq!(
            Release::from_assets("v0.2.0", page, no_digest),
            Err(ReleaseError::NoDigest(v(0, 2, 0)))
        );
    }

    #[test]
    fn only_newer_releases_are_offered() {
        let mut u = Updates::new(v(0, 2, 0));
        assert!(!u.found(None, false));
        assert!(!u.found(Some(release(v(0, 1, 9))), false), "older");
        assert!(!u.found(Some(release(v(0, 2, 0))), false), "same");
        assert_eq!(u.status(), &Status::Idle);
        assert!(u.found(Some(release(v(0, 2, 1))), false));
        assert_eq!(u.status(), &Status::Available(release(v(0, 2, 1))));
    }

    #[test]
    fn later_hides_that_version_until_a_newer_one_or_a_manual_check() {
        let mut u = Updates::new(v(0, 1, 0));
        u.found(Some(release(v(0, 2, 0))), false);
        u.dismiss();
        assert_eq!(u.status(), &Status::Idle);
        assert!(u.found(Some(release(v(0, 2, 0))), false), "still newer than this build");
        assert_eq!(u.status(), &Status::Idle, "but not offered again by the periodic check");
        u.found(Some(release(v(0, 2, 0))), true);
        assert!(matches!(u.status(), Status::Available(_)), "a manual check offers it again");
        u.dismiss();
        u.found(Some(release(v(0, 3, 0))), false);
        assert_eq!(u.status(), &Status::Available(release(v(0, 3, 0))), "a newer one is offered");
    }

    #[test]
    fn install_downloads_once_and_can_be_retried_after_a_failure() {
        let mut u = Updates::new(v(0, 1, 0));
        assert_eq!(u.start(), None, "nothing on offer");
        u.found(Some(release(v(0, 2, 0))), false);
        assert_eq!(u.start(), Some(release(v(0, 2, 0))));
        assert_eq!(u.start(), None, "already downloading");
        u.found(Some(release(v(0, 3, 0))), false);
        assert_eq!(u.status(), &Status::Downloading(release(v(0, 2, 0))), "not interrupted");
        u.failed("checksum mismatch");
        assert_eq!(u.status(), &Status::Failed(release(v(0, 2, 0)), "checksum mismatch".into()));
        assert_eq!(u.start(), Some(release(v(0, 2, 0))), "retry");
    }

    proptest! {
        #[test]
        fn version_display_round_trips(major in 0u64..10_000, minor in 0u64..10_000, patch in 0u64..10_000) {
            let version = v(major, minor, patch);
            prop_assert_eq!(Version::parse(&version.to_string()), Some(version));
            prop_assert_eq!(Version::parse(&format!("v{version}")), Some(version));
        }

        #[test]
        fn version_parse_never_panics(text in "\\PC{0,20}") {
            let _ = Version::parse(&text);
        }
    }
}

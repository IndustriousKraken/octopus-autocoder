//! a011: assisted-installer package-manager detection + the OS-package
//! dependency catalogue. The actual per-step-consent install flow lives in
//! `install.rs` (it needs the `WizardIo`/`SystemActions` traits); this module
//! holds the pure, table-driven pieces so they are unit-testable on their own.

use super::install::SystemActions;

/// A host package manager the installer knows how to drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageManager {
    /// Debian/Ubuntu `apt-get`.
    Apt,
    /// Fedora/RHEL `dnf`.
    Dnf,
    /// Arch `pacman`.
    Pacman,
    /// openSUSE `zypper`.
    Zypper,
    /// macOS Homebrew `brew`.
    Brew,
}

impl PackageManager {
    /// Every manager in detection-preference order.
    pub const ALL: [PackageManager; 5] = [
        PackageManager::Apt,
        PackageManager::Dnf,
        PackageManager::Pacman,
        PackageManager::Zypper,
        PackageManager::Brew,
    ];

    /// The manager's own binary name (also used to detect it on PATH).
    pub fn binary(self) -> &'static str {
        match self {
            PackageManager::Apt => "apt-get",
            PackageManager::Dnf => "dnf",
            PackageManager::Pacman => "pacman",
            PackageManager::Zypper => "zypper",
            PackageManager::Brew => "brew",
        }
    }

    /// The exact argv (manager binary included) that installs `pkg`. This is
    /// the command shown to the operator AND the one run on consent.
    pub fn install_argv(self, pkg: &str) -> Vec<String> {
        let parts: &[&str] = match self {
            PackageManager::Apt => &["apt-get", "install", "-y"],
            PackageManager::Dnf => &["dnf", "install", "-y"],
            PackageManager::Pacman => &["pacman", "-S", "--noconfirm"],
            PackageManager::Zypper => &["zypper", "install", "-y"],
            PackageManager::Brew => &["brew", "install"],
        };
        parts.iter().map(|s| s.to_string()).chain(std::iter::once(pkg.to_string())).collect()
    }

    /// Whether installs through this manager need elevated privilege. Homebrew
    /// runs as the invoking user; the rest need root.
    pub fn needs_privilege(self) -> bool {
        !matches!(self, PackageManager::Brew)
    }
}

/// An OS-package dependency the installer can offer to auto-install.
pub struct OsPackageDep {
    /// Operator-facing label, e.g. "bubblewrap (sandbox mechanism)".
    pub label: &'static str,
    /// The binary whose presence means the dependency is satisfied.
    pub check_bin: &'static str,
    /// The package name per manager; `None` when this manager has no package
    /// for it (e.g. bubblewrap on Homebrew — macOS uses `sandbox-exec`).
    pub pkg_name: fn(PackageManager) -> Option<&'static str>,
}

fn bubblewrap_pkg(m: PackageManager) -> Option<&'static str> {
    match m {
        PackageManager::Brew => None,
        _ => Some("bubblewrap"),
    }
}

fn git_pkg(_m: PackageManager) -> Option<&'static str> {
    Some("git")
}

fn gh_pkg(m: PackageManager) -> Option<&'static str> {
    match m {
        // Arch ships it as `github-cli`; everyone else as `gh` (apt needs the
        // GitHub apt repo first, but the package name is `gh`).
        PackageManager::Pacman => Some("github-cli"),
        _ => Some("gh"),
    }
}

/// The OS-package dependencies the installer offers, in order. Each is offered
/// with its own consent step (a011 task 2.2).
pub const OS_PACKAGE_DEPS: &[OsPackageDep] = &[
    OsPackageDep {
        label: "git",
        check_bin: "git",
        pkg_name: git_pkg,
    },
    OsPackageDep {
        label: "bubblewrap (sandbox mechanism)",
        check_bin: "bwrap",
        pkg_name: bubblewrap_pkg,
    },
    OsPackageDep {
        label: "GitHub CLI (forge/scout)",
        check_bin: "gh",
        pkg_name: gh_pkg,
    },
];

/// Detect the host package manager by probing each manager's binary on PATH,
/// in [`PackageManager::ALL`] order. `None` when none is found.
pub async fn detect(actions: &dyn SystemActions) -> Option<PackageManager> {
    for pm in PackageManager::ALL {
        if actions.which(pm.binary()).await.is_some() {
            return Some(pm);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_argv_is_per_manager_and_ends_with_pkg() {
        assert_eq!(
            PackageManager::Apt.install_argv("git"),
            vec!["apt-get", "install", "-y", "git"]
        );
        assert_eq!(
            PackageManager::Pacman.install_argv("github-cli"),
            vec!["pacman", "-S", "--noconfirm", "github-cli"]
        );
        assert_eq!(
            PackageManager::Brew.install_argv("git"),
            vec!["brew", "install", "git"]
        );
        // Always ends with the package name.
        for pm in PackageManager::ALL {
            assert_eq!(pm.install_argv("pkgx").last().unwrap(), "pkgx");
        }
    }

    #[test]
    fn brew_needs_no_privilege_others_do() {
        assert!(!PackageManager::Brew.needs_privilege());
        assert!(PackageManager::Apt.needs_privilege());
        assert!(PackageManager::Dnf.needs_privilege());
    }

    #[test]
    fn package_names_vary_by_manager() {
        assert_eq!(gh_pkg(PackageManager::Pacman), Some("github-cli"));
        assert_eq!(gh_pkg(PackageManager::Apt), Some("gh"));
        assert_eq!(bubblewrap_pkg(PackageManager::Brew), None);
        assert_eq!(bubblewrap_pkg(PackageManager::Apt), Some("bubblewrap"));
    }
}

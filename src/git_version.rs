//! The version rule: turns `git describe --tags --long` output into the version the exe is
//! built with. Shared by build.rs (which runs git) and the tests here.
//!
//! - on a tag:              `v0.6.0-0-gabc1234`    -> `0.6.0`
//! - N commits after it:    `v0.6.0-3-gabc1234`    -> `0.6.1-dev.3+gabc1234`
//! - after a pre-release:   `v1.0.0-rc.1-3-gabc12` -> `1.0.0-rc.1.dev.3+gabc12`
//!
//! Dev versions sort after the tag they follow and before the next release.

/// The version for `describe`, or `None` if it isn't `v<version>-<count>-g<hash>`.
pub fn from_describe(describe: &str) -> Option<String> {
    // Split from the right: the tag itself may contain dashes (v1.0.0-rc.1).
    let mut parts = describe.trim().rsplitn(3, '-');
    let hash = parts.next()?.strip_prefix('g')?;
    let count: u32 = parts.next()?.parse().ok()?;
    let tag = parts.next()?.strip_prefix('v')?;

    let (core, pre) = match tag.split_once('-') {
        Some((c, p)) => (c, Some(p)),
        None => (tag, None),
    };
    let nums: Vec<u64> = core.split('.').map(|n| n.parse().ok()).collect::<Option<_>>()?;
    if nums.len() != 3 || hash.is_empty() || !hash.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    if pre.is_some_and(|p| p.is_empty()) {
        return None;
    }

    if count == 0 {
        return Some(tag.to_string());
    }
    Some(match pre {
        Some(pre) => format!("{core}-{pre}.dev.{count}+g{hash}"),
        None => format!("{}.{}.{}-dev.{count}+g{hash}", nums[0], nums[1], nums[2] + 1),
    })
}

/// The four numbers for the Windows version resource: X.Y.Z plus the dev count.
pub fn file_version(version: &str) -> [u16; 4] {
    let (main, _build) = version.split_once('+').unwrap_or((version, ""));
    let (core, pre) = main.split_once('-').unwrap_or((main, ""));
    let mut out = [0u16; 4];
    for (slot, n) in out.iter_mut().zip(core.split('.')) {
        *slot = n.parse().unwrap_or(0);
    }
    // "dev.3" or "rc.1.dev.3": the number after "dev".
    let parts: Vec<&str> = pre.split('.').collect();
    if let Some(i) = parts.iter().position(|p| *p == "dev") {
        out[3] = parts.get(i + 1).and_then(|n| n.parse().ok()).unwrap_or(0);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_tag() {
        assert_eq!(from_describe("v0.6.0-0-gabc1234\n").as_deref(), Some("0.6.0"));
        assert_eq!(from_describe("v1.0.0-rc.1-0-gabc1234").as_deref(), Some("1.0.0-rc.1"));
    }

    #[test]
    fn after_tag() {
        assert_eq!(from_describe("v0.6.0-3-gabc1234").as_deref(), Some("0.6.1-dev.3+gabc1234"));
        assert_eq!(
            from_describe("v1.0.0-rc.1-12-gabc1234").as_deref(),
            Some("1.0.0-rc.1.dev.12+gabc1234")
        );
    }

    #[test]
    fn not_a_version() {
        for d in ["Release-0-gabc", "v1.2-3-gabc", "v1.2.3", "v1.2.3-x-gabc", "v1.2.3-1-gxyz", ""] {
            assert_eq!(from_describe(d), None, "{d}");
        }
    }

    #[test]
    fn windows_file_version() {
        assert_eq!(file_version("0.6.0"), [0, 6, 0, 0]);
        assert_eq!(file_version("0.6.1-dev.3+gabc"), [0, 6, 1, 3]);
        assert_eq!(file_version("1.0.0-rc.1.dev.7+gabc"), [1, 0, 0, 7]);
        assert_eq!(file_version("1.0.0-rc.1"), [1, 0, 0, 0]);
    }
}

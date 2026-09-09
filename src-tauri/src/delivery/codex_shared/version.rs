//! Version eligibility is separate from protocol capability validation. Newer
//! stable versions and the exact tested alpha must complete the owned, no-turn handshake;
//! eligibility never means that a future release has passed provider acceptance.
use super::CodexSharedError;

pub(super) fn supported_version(user_agent: &str) -> Result<String, CodexSharedError> {
    let reject = || {
        CodexSharedError::unsupported(
        "shared Codex requires stable CLI >=0.154.0 or exact 0.154.0-alpha.6 plus compatible initialize, direct thread input and thread/inject_items; no v2 composer fallback"
    )
    };
    let version = user_agent
        .split_whitespace()
        .next()
        .and_then(|value| value.strip_prefix("wardian/"))
        .ok_or_else(reject)?;
    // The isolated Windows local-daemon probe tested this exact official alpha
    // with a local model stub. This is eligibility, not Wardian acceptance.
    // Match the entire version so build-tagged or adjacent alphas stay rejected.
    if version == "0.154.0-alpha.6" {
        return Ok(version.to_owned());
    }
    let mut parts = version.split('+');
    let core = parts.next().ok_or_else(reject)?;
    if let Some(build) = parts.next() {
        if build.split('.').any(|part| {
            part.is_empty()
                || !part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-')
        }) {
            return Err(reject());
        }
    }
    if parts.next().is_some() {
        return Err(reject());
    }
    let numbers = core
        .split('.')
        .map(|part| {
            if part.is_empty()
                || (part.len() > 1 && part.starts_with('0'))
                || !part.bytes().all(|byte| byte.is_ascii_digit())
            {
                None
            } else {
                part.parse::<u64>().ok()
            }
        })
        .collect::<Option<Vec<_>>>()
        .ok_or_else(reject)?;
    if numbers.len() != 3 || (numbers[0], numbers[1], numbers[2]) < (0, 154, 0) {
        return Err(reject());
    }
    Ok(version.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_exact_tested_alpha_is_eligible() {
        assert_eq!(
            supported_version("wardian/0.154.0-alpha.6 (test platform)").unwrap(),
            "0.154.0-alpha.6"
        );
        for version in [
            "0.154.0-alpha.5",
            "0.154.0-alpha.7",
            "0.154.0-alpha.06",
            "0.154.0-alpha.6+build.1",
            "0.154.0-beta.6",
            "0.154.1-alpha.6",
            "1.0.0-alpha.6",
        ] {
            assert!(
                supported_version(&format!("wardian/{version}")).is_err(),
                "{version}"
            );
        }
        assert!(supported_version("foreign/0.154.0-alpha.6").is_err());
    }

    #[test]
    fn stable_versions_before_local_daemon_minimum_are_rejected() {
        for version in [
            "0.153.4",
            "0.153.5",
            "0.153.99",
            "0.153.4+build.7",
            "0.153.5+build.7",
            "0.153.99+build.7",
        ] {
            let error = supported_version(&format!("wardian/{version} (test platform)"))
                .expect_err(version);
            assert!(error.to_string().contains("stable CLI >=0.154.0"));
        }
    }

    #[test]
    fn stable_updates_are_eligible_without_claiming_capability_support() {
        for version in ["0.154.0", "0.154.1", "0.155.0", "1.0.0", "0.154.0+build.7"] {
            assert_eq!(
                supported_version(&format!("wardian/{version} (test platform)")).unwrap(),
                version
            );
        }
        for version in [
            "0.153.3",
            "0.152.99",
            "0.153",
            "0.154.0-alpha",
            "unknown",
            "00.154.0",
            "0.154.0+",
            "0.154.0+a+b",
            "0.154.18446744073709551616",
        ] {
            assert!(
                supported_version(&format!("wardian/{version}")).is_err(),
                "{version}"
            );
        }
        assert!(supported_version("foreign/0.154.0").is_err());
    }
}

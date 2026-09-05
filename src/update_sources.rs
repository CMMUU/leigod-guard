//! User-approved update policy: complete stable versions first, domestic source
//! first on a tie, and a single verified fallback for the exact selected release.
use super::*;

pub(super) const CHECK_TIMEOUT: Duration = Duration::from_secs(8);

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum UpdateMode {
    #[default]
    Auto,
    GitHub,
    Gitee,
}

impl UpdateMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "自动选择（国内优先）",
            Self::Gitee => "仅 Gitee（国内）",
            Self::GitHub => "仅 GitHub",
        }
    }

    fn source(self) -> Option<UpdateSource> {
        match self {
            Self::Auto => None,
            Self::Gitee => Some(UpdateSource::Gitee),
            Self::GitHub => Some(UpdateSource::GitHub),
        }
    }
}

#[derive(Clone, Debug)]
pub struct UpdatePlan {
    pub release: ReleaseInfo,
    backup: Option<ReleaseInfo>,
    automatic: bool,
}

impl UpdatePlan {
    pub fn automatic(&self) -> bool {
        self.automatic
    }
}

#[derive(Debug)]
pub struct UpdateCheck {
    pub plan: Option<UpdatePlan>,
    pub message: String,
    /// At least one source is unknown, even if the other has a usable update.
    pub partial: bool,
}

type SourceResult = Result<Option<ReleaseInfo>, String>;

pub fn check_for_updates(current: &str, mode: UpdateMode) -> Result<UpdateCheck, String> {
    check_using(current, mode, |source, deadline| {
        check_latest_before(current, source, deadline)
    })
}

fn check_using(
    current: &str,
    mode: UpdateMode,
    check: impl Fn(UpdateSource, Instant) -> SourceResult + Sync,
) -> Result<UpdateCheck, String> {
    parse_version(current)?;
    let deadline = Instant::now() + CHECK_TIMEOUT;
    if let Some(source) = mode.source() {
        let release = check(source, deadline)?;
        validate_candidate(current, source, &release)?;
        return Ok(UpdateCheck {
            message: release.as_ref().map_or_else(
                || format!("{} 暂无更高的正式版本。", mode.label()),
                |r| format!("{} 发现新版本 v{}。", source.label(), r.version),
            ),
            plan: release.map(|release| UpdatePlan {
                release,
                backup: None,
                automatic: false,
            }),
            partial: false,
        });
    }
    // Both share one deadline. Joining does not add two serial HTTP timeouts.
    let (gitee, github) = std::thread::scope(|scope| {
        let gitee = scope.spawn(|| check(UpdateSource::Gitee, deadline));
        let github = scope.spawn(|| check(UpdateSource::GitHub, deadline));
        (
            gitee
                .join()
                .unwrap_or_else(|_| Err("Gitee 检查未能完成。".into())),
            github
                .join()
                .unwrap_or_else(|_| Err("GitHub 检查未能完成。".into())),
        )
    });
    select_update(current, gitee, github)
}

fn validate_candidate(
    current: &str,
    source: UpdateSource,
    candidate: &Option<ReleaseInfo>,
) -> Result<(), String> {
    if let Some(release) = candidate {
        if release.source != source
            || release.tag != format!("v{}", release.version)
            || parse_version(&release.version)? <= parse_version(current)?
        {
            return Err("来源返回的版本信息不一致，请重新检查。".into());
        }
    }
    Ok(())
}

fn select_update(
    current: &str,
    gitee: SourceResult,
    github: SourceResult,
) -> Result<UpdateCheck, String> {
    let failed: Vec<_> = [
        (UpdateSource::Gitee, &gitee),
        (UpdateSource::GitHub, &github),
    ]
    .into_iter()
    .filter(|(_, result)| result.is_err())
    .map(|(source, _)| source.label())
    .collect();
    if failed.len() == 2 {
        return Err("Gitee 和 GitHub 均未能完成检查，无法确认是否有新版本。请稍后重试，或使用下方手动下载入口。".into());
    }
    let gitee = gitee.ok().flatten();
    let github = github.ok().flatten();
    validate_candidate(current, UpdateSource::Gitee, &gitee)?;
    validate_candidate(current, UpdateSource::GitHub, &github)?;
    let (release, backup) = match (gitee, github) {
        (Some(gitee), Some(github)) => {
            match parse_version(&gitee.version)?.cmp(&parse_version(&github.version)?) {
                std::cmp::Ordering::Equal => (Some(gitee), Some(github)),
                std::cmp::Ordering::Greater => (Some(gitee), None),
                std::cmp::Ordering::Less => (Some(github), None),
            }
        }
        (Some(release), None) | (None, Some(release)) => (Some(release), None),
        (None, None) => (None, None),
    };
    let mut message = match &release {
        Some(release) if release.source == UpdateSource::Gitee => {
            format!(
                "发现新版本 v{}，优先从 Gitee（国内）下载。",
                release.version
            )
        }
        Some(release) => format!(
            "发现新版本 v{}；国内源暂无同版本可用包，本次使用 GitHub。",
            release.version
        ),
        None if failed.is_empty() => "两个来源均未发现更高的正式版本。".into(),
        None => "可访问的来源暂未发现更高的正式版本。".into(),
    };
    if !failed.is_empty() {
        message.push_str(&format!(
            " {} 未能完成检查，该来源是否有更新尚未确认。",
            failed.join("、")
        ));
    }
    Ok(UpdateCheck {
        plan: release.map(|release| UpdatePlan {
            release,
            backup,
            automatic: true,
        }),
        message,
        partial: !failed.is_empty(),
    })
}

fn other_source(source: UpdateSource) -> UpdateSource {
    match source {
        UpdateSource::Gitee => UpdateSource::GitHub,
        UpdateSource::GitHub => UpdateSource::Gitee,
    }
}

fn package_asset(release: &ReleaseInfo, kind: PackageKind) -> &Asset {
    match kind {
        PackageKind::Installer => &release.installer,
        PackageKind::Portable => &release.portable,
    }
}

fn validate_backup(
    primary: &ReleaseInfo,
    backup: &ReleaseInfo,
    kind: PackageKind,
) -> Result<(), String> {
    let original = package_asset(primary, kind);
    let alternate = package_asset(backup, kind);
    if backup.source != other_source(primary.source)
        || primary.tag != format!("v{}", primary.version)
        || backup.tag != primary.tag
        || backup.version != primary.version
        || original.name != package_name(&primary.tag, kind)
        || alternate.name != original.name
        || alternate.size != original.size
    {
        return Err("备用源的版本、安装方式或文件信息不一致，已停止切换，请重新检查更新。".into());
    }
    Ok(())
}

/// The callbacks publish status only. Nothing here launches an installer.
pub fn download_planned_update(
    plan: &UpdatePlan,
    kind: PackageKind,
    staging_dir: &Path,
    progress: &dyn Fn(DownloadProgress),
    source_changed: &dyn Fn(UpdateSource, bool),
) -> Result<DownloadedUpdate, String> {
    download_using(plan, kind, source_changed, check_tag, |release, pin| {
        download_inner(release, kind, staging_dir, pin, progress)
    })
}

fn download_using(
    plan: &UpdatePlan,
    kind: PackageKind,
    source_changed: &dyn Fn(UpdateSource, bool),
    mut lookup: impl FnMut(&str, UpdateSource) -> SourceResult,
    mut attempt: impl FnMut(&ReleaseInfo, &mut Option<String>) -> Result<DownloadedUpdate, String>,
) -> Result<DownloadedUpdate, String> {
    parse_version(&plan.release.version)?;
    if plan.release.tag != format!("v{}", plan.release.version) {
        return Err("更新目标版本不一致，请重新检查更新。".into());
    }
    let mut pin = None;
    if let Some(backup) = &plan.backup {
        validate_backup(&plan.release, backup, kind)?;
        // A known GitHub API digest also constrains the preferred Gitee copy.
        if let Some(digest) = &package_asset(backup, kind).digest {
            pin_checksum(&mut pin, &parse_api_digest(digest)?)?;
        }
    }
    source_changed(plan.release.source, false);
    let first_error = match attempt(&plan.release, &mut pin) {
        Ok(downloaded) => return Ok(downloaded),
        Err(message) if !plan.automatic => {
            return Err(with_manual_fallback(message, plan.release.source))
        }
        Err(message) => message,
    };
    let source = other_source(plan.release.source);
    source_changed(source, true);
    let backup = match &plan.backup {
        Some(backup) => backup.clone(),
        None => lookup(&plan.release.tag, source)
            .map_err(|_| format!("{} 下载未完成：{first_error}\n备用源 {} 未能确认同版本文件，请稍后重试。", plan.release.source.label(), source.label()))?
            .ok_or_else(|| format!("{} 下载未完成：{first_error}\n备用源 {} 暂无 {}，已停止更新，不会改装其他版本。", plan.release.source.label(), source.label(), plan.release.tag))?,
    };
    validate_backup(&plan.release, &backup, kind)?;
    attempt(&backup, &mut pin).map_err(|message| format!(
        "{} 下载未完成：{first_error}\n{} 备用下载也未完成：{message}\n请稍后重试，或使用手动下载入口。",
        plan.release.source.label(), backup.source.label()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::sync::{Condvar, Mutex};

    fn release(source: UpdateSource, tag: &str) -> ReleaseInfo {
        if source == UpdateSource::GitHub {
            super::super::tests::parse_fixture(&super::super::tests::fixture_release(tag), "0.0.0")
                .unwrap()
                .unwrap()
        } else {
            let (release, assets) = super::super::tests::gitee_fixture(tag);
            super::super::tests::parse_gitee_fixture(release, assets).unwrap()
        }
    }

    fn downloaded(kind: PackageKind, version: &str) -> DownloadedUpdate {
        DownloadedUpdate {
            kind,
            version: version.into(),
            path: PathBuf::from("fixture-not-executed"),
            sha256: "a".repeat(64),
            size: 100,
        }
    }

    #[test]
    #[ignore = "explicit anonymous public metadata check; no package download or application launch"]
    fn public_auto_check_and_exact_tag_lookup() {
        let started = Instant::now();
        let report = check_for_updates("0.0.0", UpdateMode::Auto).unwrap();
        println!(
            "Check completed in {:?}: {}",
            started.elapsed(),
            report.message
        );
        let plan = report
            .plan
            .expect("at least one public stable release exists");
        let exact = check_tag(&plan.release.tag, plan.release.source)
            .unwrap()
            .unwrap();
        assert_eq!(exact.tag, plan.release.tag);
        assert_eq!(exact.version, plan.release.version);
        assert_eq!(exact.installer.size, plan.release.installer.size);
        assert_eq!(exact.portable.size, plan.release.portable.size);
        println!(
            "Verified exact tag {} from {}",
            exact.tag,
            exact.source.label()
        );
    }

    #[test]
    fn auto_checks_sources_concurrently_and_prefers_domestic_for_the_same_version() {
        let started = (Mutex::new(0), Condvar::new());
        let deadlines = Mutex::new(Vec::new());
        let report = check_using("0.8.0", UpdateMode::Auto, |source, deadline| {
            deadlines.lock().unwrap().push(deadline);
            let (count, wake) = &started;
            let mut count = count.lock().unwrap();
            *count += 1;
            wake.notify_all();
            let (_guard, timeout) = wake
                .wait_timeout_while(count, Duration::from_secs(2), |count| *count < 2)
                .unwrap();
            assert!(!timeout.timed_out(), "source checks must run concurrently");
            Ok(Some(release(source, "v0.9.0")))
        })
        .unwrap();
        let plan = report.plan.unwrap();
        assert_eq!(plan.release.source, UpdateSource::Gitee);
        assert_eq!(plan.backup.unwrap().source, UpdateSource::GitHub);
        assert!(!report.partial);
        let deadlines = deadlines.lock().unwrap();
        assert_eq!(deadlines[0], deadlines[1]);
    }

    #[test]
    fn versions_win_over_source_priority_and_an_old_version_never_becomes_a_backup() {
        let report = select_update(
            "0.7.0",
            Ok(Some(release(UpdateSource::Gitee, "v0.8.0"))),
            Ok(Some(release(UpdateSource::GitHub, "v0.9.0"))),
        )
        .unwrap();
        let plan = report.plan.unwrap();
        assert_eq!(plan.release.source, UpdateSource::GitHub);
        assert!(plan.backup.is_none());
        assert!(select_update(
            "0.9.0",
            Ok(Some(release(UpdateSource::Gitee, "v0.8.0"))),
            Ok(None)
        )
        .is_err());
    }

    #[test]
    fn unreachable_sources_never_become_an_up_to_date_result() {
        let report = select_update("0.9.0", Ok(None), Err("offline".into())).unwrap();
        assert!(report.partial && report.plan.is_none());
        assert!(report.message.contains("未能完成检查"));
        assert!(!report.message.contains("已是最新"));
        assert!(select_update("0.9.0", Err("a".into()), Err("b".into())).is_err());
        let report = select_update(
            "0.8.0",
            Ok(Some(release(UpdateSource::Gitee, "v0.9.0"))),
            Err("offline".into()),
        )
        .unwrap();
        assert_eq!(report.plan.unwrap().release.source, UpdateSource::Gitee);
        assert!(report.partial);
    }

    #[test]
    fn manual_modes_query_and_download_only_the_requested_source() {
        for mode in [UpdateMode::Gitee, UpdateMode::GitHub] {
            let calls = Mutex::new(Vec::new());
            let report = check_using("0.8.0", mode, |source, _| {
                calls.lock().unwrap().push(source);
                Ok(Some(release(source, "v0.9.0")))
            })
            .unwrap();
            assert_eq!(*calls.lock().unwrap(), vec![mode.source().unwrap()]);
            let mut attempts = 0;
            assert!(download_using(
                &report.plan.unwrap(),
                PackageKind::Installer,
                &|_, _| {},
                |_, _| panic!("manual mode must not contact a backup"),
                |_, _| {
                    attempts += 1;
                    Err("fixture failure".into())
                }
            )
            .is_err());
            assert_eq!(attempts, 1);
        }
    }

    #[test]
    fn fallback_keeps_the_exact_version_package_kind_and_checksum() {
        for kind in [PackageKind::Installer, PackageKind::Portable] {
            let plan = UpdatePlan {
                release: release(UpdateSource::Gitee, "v0.9.0"),
                backup: None,
                automatic: true,
            };
            let changes = RefCell::new(Vec::new());
            let mut attempts = Vec::new();
            let result = download_using(
                &plan,
                kind,
                &|source, switched| changes.borrow_mut().push((source, switched)),
                |tag, source| {
                    assert_eq!(tag, "v0.9.0");
                    assert_eq!(source, UpdateSource::GitHub);
                    Ok(Some(release(source, tag)))
                },
                |release, pin| {
                    attempts.push(release.source);
                    pin_checksum(pin, &"a".repeat(64))?;
                    if release.source == UpdateSource::Gitee {
                        return Err("connection interrupted".into());
                    }
                    assert_eq!(
                        package_asset(release, kind).name,
                        package_name("v0.9.0", kind)
                    );
                    Ok(downloaded(kind, &release.version))
                },
            )
            .unwrap();
            assert_eq!(result.kind, kind);
            assert_eq!(result.version, "0.9.0");
            assert_eq!(attempts, vec![UpdateSource::Gitee, UpdateSource::GitHub]);
            assert_eq!(
                *changes.borrow(),
                vec![(UpdateSource::Gitee, false), (UpdateSource::GitHub, true)]
            );
        }
    }

    #[test]
    fn fallback_refuses_different_versions_sizes_or_bytes_and_never_loops() {
        let plan = UpdatePlan {
            release: release(UpdateSource::Gitee, "v0.9.0"),
            backup: None,
            automatic: true,
        };
        for mismatch in 0..3 {
            let mut calls = 0;
            let result = download_using(
                &plan,
                PackageKind::Portable,
                &|_, _| {},
                |_, source| {
                    let mut backup =
                        release(source, if mismatch == 0 { "v0.10.0" } else { "v0.9.0" });
                    if mismatch == 1 {
                        backup.portable.size += 1;
                    }
                    Ok(Some(backup))
                },
                |_, pin| {
                    calls += 1;
                    pin_checksum(
                        pin,
                        &if calls == 1 {
                            "a".repeat(64)
                        } else {
                            "b".repeat(64)
                        },
                    )?;
                    Err("fixture failure".into())
                },
            );
            assert!(result.is_err());
            assert_eq!(calls, if mismatch == 2 { 2 } else { 1 });
        }
    }

    #[test]
    fn successful_primary_never_probes_or_downloads_a_backup() {
        let plan = UpdatePlan {
            release: release(UpdateSource::Gitee, "v0.9.0"),
            backup: None,
            automatic: true,
        };
        let result = download_using(
            &plan,
            PackageKind::Installer,
            &|_, _| {},
            |_, _| panic!("no fallback needed"),
            |_, _| Ok(downloaded(PackageKind::Installer, "0.9.0")),
        );
        assert!(result.is_ok());
    }

    #[test]
    fn missing_exact_tag_stops_after_primary_failure() {
        let plan = UpdatePlan {
            release: release(UpdateSource::GitHub, "v0.9.0"),
            backup: None,
            automatic: true,
        };
        let mut attempts = 0;
        let error = download_using(
            &plan,
            PackageKind::Installer,
            &|_, _| {},
            |tag, source| {
                assert_eq!(tag, "v0.9.0");
                assert_eq!(source, UpdateSource::Gitee);
                Ok(None)
            },
            |_, _| {
                attempts += 1;
                Err("timeout".into())
            },
        )
        .unwrap_err();
        assert_eq!(attempts, 1);
        assert!(error.contains("不会改装其他版本"));
    }

    #[test]
    fn known_api_digest_constrains_domestic_payload_before_download() {
        let primary = release(UpdateSource::Gitee, "v0.9.0");
        let mut backup = release(UpdateSource::GitHub, "v0.9.0");
        backup.installer.digest = Some(format!("sha256:{}", "a".repeat(64)));
        let plan = UpdatePlan {
            release: primary,
            backup: Some(backup),
            automatic: true,
        };
        let mut attempts = Vec::new();
        let result = download_using(
            &plan,
            PackageKind::Installer,
            &|_, _| {},
            |_, _| panic!("backup already known"),
            |release, pin| {
                attempts.push(release.source);
                assert_eq!(pin.as_deref(), Some("a".repeat(64).as_str()));
                let reported = if release.source == UpdateSource::Gitee {
                    "b".repeat(64)
                } else {
                    "a".repeat(64)
                };
                pin_checksum(pin, &reported)?;
                Ok(downloaded(PackageKind::Installer, &release.version))
            },
        )
        .unwrap();
        assert_eq!(attempts, vec![UpdateSource::Gitee, UpdateSource::GitHub]);
        assert_eq!(result.sha256, "a".repeat(64));
    }
}

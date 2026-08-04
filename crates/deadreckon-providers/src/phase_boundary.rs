use std::path::{Path, PathBuf};
use std::time::Duration;

use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::{
    ProviderError, ProviderKind, ProviderProcessLifetime, ProviderRequest, ProviderResponse,
    ProviderRouter, Result,
};

/// One absolute work deadline followed by a separate, bounded cleanup window.
///
/// The cleanup budget never grants more provider work. It exists only to let
/// the already-running provider future observe cancellation, reap its process
/// tree, and compare-remove its PID authority record.
#[derive(Clone, Copy, Debug)]
pub struct ProviderPhaseDeadline {
    pub work_expires_at: Instant,
    pub cleanup_budget: Duration,
}

impl ProviderPhaseDeadline {
    pub fn new(work_expires_at: Instant, cleanup_budget: Duration) -> Self {
        Self {
            work_expires_at,
            cleanup_budget,
        }
    }

    pub fn from_now(work_budget: Duration, cleanup_budget: Duration) -> Self {
        Self::new(Instant::now() + work_budget, cleanup_budget)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderCleanup {
    /// The provider future resolved and its PID authority path is absent.
    Proven,
    /// The route owns no operating-system process authority (for example an
    /// HTTP provider), so dropping its transport future completes cleanup.
    NotApplicable,
    /// Cleanup did not complete, or its authority record could not be proven
    /// absent. The record is deliberately retained for durable reconciliation.
    RetainedAuthority { path: PathBuf, detail: String },
}

#[derive(Debug)]
pub enum ProviderPhaseOutcome<T> {
    Completed(T),
    WorkExpired { cleanup: ProviderCleanup },
    Cancelled { cleanup: ProviderCleanup },
}

enum ProviderPhaseBoundary<T> {
    Completed(T),
    WorkExpired,
    Cancelled,
}

/// Complete one provider phase without allowing a work timeout or external
/// cancellation to drop a live CLI silently.
///
/// Any cancellation token already present on `request` is treated as the
/// external controller signal. The provider receives a distinct child token,
/// which this boundary cancels exactly once before entering cleanup.
pub async fn complete_provider_phase(
    router: &ProviderRouter,
    request: &mut ProviderRequest,
    deadline: ProviderPhaseDeadline,
) -> ProviderPhaseOutcome<Result<ProviderResponse>> {
    let route_lifetimes = router
        .routes()
        .iter()
        .map(|route| (route.name().to_string(), route.process_lifetime(request)))
        .collect::<Vec<_>>();
    let mut phase_request = request.clone();
    if phase_request.pid_file.is_none()
        && router
            .routes()
            .iter()
            .any(|route| provider_kind_uses_process(&route.kind()))
    {
        phase_request.pid_file = Some(fresh_process_authority_path());
    }
    let external_cancellation = request.cancellation_token.clone();
    let provider_cancellation = CancellationToken::new();
    phase_request.cancellation_token = Some(provider_cancellation.clone());
    let authority = phase_request.pid_file.clone();

    if external_cancellation
        .as_ref()
        .is_some_and(CancellationToken::is_cancelled)
    {
        return ProviderPhaseOutcome::Cancelled {
            cleanup: cleanup_without_started_work(authority.as_deref()),
        };
    }
    if Instant::now() >= deadline.work_expires_at {
        return ProviderPhaseOutcome::WorkExpired {
            cleanup: cleanup_without_started_work(authority.as_deref()),
        };
    }

    let completion = router.complete(&phase_request);
    tokio::pin!(completion);
    let boundary = if let Some(external) = external_cancellation.as_ref() {
        tokio::select! {
            biased;
            () = external.cancelled() => ProviderPhaseBoundary::Cancelled,
            result = &mut completion => ProviderPhaseBoundary::Completed(result),
            () = tokio::time::sleep_until(deadline.work_expires_at) => {
                ProviderPhaseBoundary::WorkExpired
            }
        }
    } else {
        tokio::select! {
            result = &mut completion => ProviderPhaseBoundary::Completed(result),
            () = tokio::time::sleep_until(deadline.work_expires_at) => {
                ProviderPhaseBoundary::WorkExpired
            }
        }
    };

    match boundary {
        ProviderPhaseBoundary::Completed(result) => ProviderPhaseOutcome::Completed(
            prove_completed_cleanup(result, authority.as_deref(), &route_lifetimes),
        ),
        ProviderPhaseBoundary::WorkExpired => {
            provider_cancellation.cancel();
            let cleanup_resolved = tokio::time::timeout(deadline.cleanup_budget, &mut completion)
                .await
                .is_ok();
            ProviderPhaseOutcome::WorkExpired {
                cleanup: classify_cleanup(
                    authority.as_deref(),
                    cleanup_resolved,
                    deadline.cleanup_budget,
                ),
            }
        }
        ProviderPhaseBoundary::Cancelled => {
            provider_cancellation.cancel();
            let cleanup_resolved = tokio::time::timeout(deadline.cleanup_budget, &mut completion)
                .await
                .is_ok();
            ProviderPhaseOutcome::Cancelled {
                cleanup: classify_cleanup(
                    authority.as_deref(),
                    cleanup_resolved,
                    deadline.cleanup_budget,
                ),
            }
        }
    }
}

fn prove_completed_cleanup(
    result: Result<ProviderResponse>,
    authority: Option<&Path>,
    route_lifetimes: &[(String, ProviderProcessLifetime)],
) -> Result<ProviderResponse> {
    let process_lifetime = result
        .as_ref()
        .ok()
        .and_then(|response| {
            route_lifetimes
                .iter()
                .find(|(name, _)| name == &response.provider)
                .map(|(_, lifetime)| *lifetime)
        })
        .unwrap_or(ProviderProcessLifetime::Invocation);
    if process_lifetime == ProviderProcessLifetime::RouterSession {
        return result;
    }
    let Some(authority) = authority else {
        return result;
    };
    match std::fs::symlink_metadata(authority) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => result,
        Ok(_) => Err(ProviderError::CleanupIncomplete {
            provider: provider_name(&result),
            authority: Some(authority.to_path_buf()),
            detail: "provider completion returned before PID authority removal".to_string(),
        }),
        Err(error) => Err(ProviderError::CleanupIncomplete {
            provider: provider_name(&result),
            authority: Some(authority.to_path_buf()),
            detail: format!("could not inspect PID authority after completion: {error}"),
        }),
    }
}

fn provider_kind_uses_process(kind: &ProviderKind) -> bool {
    matches!(kind, ProviderKind::CliClaudeCode | ProviderKind::CliCodex)
        || matches!(kind, ProviderKind::Generic(_))
}

pub(crate) fn fresh_process_authority_path() -> PathBuf {
    std::env::temp_dir().join(format!(
        "deadreckon-provider-phase-{}.pid",
        uuid::Uuid::new_v4().simple()
    ))
}

fn provider_name(result: &Result<ProviderResponse>) -> String {
    result
        .as_ref()
        .map(|response| response.provider.clone())
        .unwrap_or_else(|_| "configured provider".to_string())
}

fn cleanup_without_started_work(authority: Option<&Path>) -> ProviderCleanup {
    let Some(authority) = authority else {
        return ProviderCleanup::NotApplicable;
    };
    match std::fs::symlink_metadata(authority) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => ProviderCleanup::Proven,
        Ok(_) => ProviderCleanup::RetainedAuthority {
            path: authority.to_path_buf(),
            detail: "work did not start, but pre-existing process authority remains".to_string(),
        },
        Err(error) => ProviderCleanup::RetainedAuthority {
            path: authority.to_path_buf(),
            detail: format!("could not inspect process authority: {error}"),
        },
    }
}

fn classify_cleanup(
    authority: Option<&Path>,
    cleanup_resolved: bool,
    cleanup_budget: Duration,
) -> ProviderCleanup {
    let Some(authority) = authority else {
        return ProviderCleanup::NotApplicable;
    };
    match std::fs::symlink_metadata(authority) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && cleanup_resolved => {
            ProviderCleanup::Proven
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            ProviderCleanup::RetainedAuthority {
                path: authority.to_path_buf(),
                detail: format!(
                    "provider cleanup did not resolve within {:.1}s; PID authority disappeared but process-tree cleanup was not observed",
                    cleanup_budget.as_secs_f64()
                ),
            }
        }
        Ok(_) => ProviderCleanup::RetainedAuthority {
            path: authority.to_path_buf(),
            detail: if cleanup_resolved {
                "provider cleanup resolved without removing PID authority".to_string()
            } else {
                format!(
                    "provider cleanup did not resolve within {:.1}s",
                    cleanup_budget.as_secs_f64()
                )
            },
        },
        Err(error) => ProviderCleanup::RetainedAuthority {
            path: authority.to_path_buf(),
            detail: format!("could not inspect process authority after cleanup: {error}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    struct DescendantGuard(PathBuf);

    #[cfg(unix)]
    impl Drop for DescendantGuard {
        fn drop(&mut self) {
            let Ok(raw) = std::fs::read_to_string(&self.0) else {
                return;
            };
            let Ok(pid) = raw.trim().parse::<u32>() else {
                return;
            };
            if deadreckon_core::pid_is_alive(pid) {
                let _ = deadreckon_core::terminate_pid(pid, true);
            }
        }
    }

    #[test]
    fn absolute_deadline_does_not_reset_when_reused() {
        let expires = Instant::now() + Duration::from_secs(30);
        let deadline = ProviderPhaseDeadline::new(expires, Duration::from_secs(3));
        assert_eq!(deadline.work_expires_at, expires);
        assert_eq!(deadline.cleanup_budget, Duration::from_secs(3));
    }

    #[test]
    fn retained_authority_is_not_collapsed_into_clean_timeout() {
        let temp = tempfile::tempdir().expect("tempdir");
        let authority = temp.path().join("provider.pid");
        std::fs::write(&authority, "retained").expect("authority");
        assert!(matches!(
            classify_cleanup(Some(&authority), false, Duration::from_millis(10)),
            ProviderCleanup::RetainedAuthority { path, .. } if path == authority
        ));
    }

    #[tokio::test]
    async fn phase_completion_preserves_caller_cancellation_for_reuse() {
        let router = ProviderRouter::smoke();
        let cancellation = CancellationToken::new();
        let mut request = ProviderRequest {
            prompt: "first phase".to_string(),
            cancellation_token: Some(cancellation.clone()),
            ..ProviderRequest::default()
        };

        let first = complete_provider_phase(
            &router,
            &mut request,
            ProviderPhaseDeadline::from_now(Duration::from_secs(1), Duration::from_secs(1)),
        )
        .await;
        assert!(matches!(first, ProviderPhaseOutcome::Completed(Ok(_))));
        assert!(
            !request
                .cancellation_token
                .as_ref()
                .expect("caller cancellation")
                .is_cancelled()
        );

        cancellation.cancel();
        assert!(
            request
                .cancellation_token
                .as_ref()
                .expect("caller cancellation")
                .is_cancelled(),
            "phase completion replaced the caller-owned cancellation token"
        );
        let second = complete_provider_phase(
            &router,
            &mut request,
            ProviderPhaseDeadline::from_now(Duration::from_secs(1), Duration::from_secs(1)),
        )
        .await;
        assert!(matches!(
            second,
            ProviderPhaseOutcome::Cancelled {
                cleanup: ProviderCleanup::NotApplicable
            }
        ));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn work_deadline_reaps_cli_tree_and_proves_pid_authority_removed() {
        use std::collections::BTreeMap;
        use std::os::unix::fs::PermissionsExt as _;

        use crate::{ProviderConfigFile, ProviderEntry, ProviderKind};

        let temp = tempfile::tempdir().expect("tempdir");
        let binary = temp.path().join("hanging-provider");
        let descendant_path = temp.path().join("descendant.pid");
        let _descendant_guard = DescendantGuard(descendant_path.clone());
        std::fs::write(
            &binary,
            format!(
                "#!/bin/sh\nif [ \"${{1:-}}\" = '--help' ]; then\n  echo '--output-format --permission-mode'\n  exit 0\nfi\n(trap '' TERM; sleep 30) &\ndescendant=$!\nprintf '%s\\n' \"$descendant\" > '{}'\ntrap '' TERM\nwait\n",
                descendant_path.display()
            ),
        )
        .expect("fake provider");
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700))
            .expect("fake provider permissions");
        let router = ProviderRouter::from_config(
            ProviderConfigFile {
                default_provider: Some("cli:claude-code".to_string()),
                fallback: None,
                providers: BTreeMap::from([(
                    "cli:claude-code".to_string(),
                    ProviderEntry {
                        kind: Some(ProviderKind::CliClaudeCode),
                        api_key: None,
                        api_key_env: None,
                        base_url: None,
                        model: None,
                        input_cost_per_million: None,
                        output_cost_per_million: None,
                        binary: Some(binary.display().to_string()),
                        extra_args: Vec::new(),
                    },
                )]),
            },
            None,
        )
        .expect("router");
        let mut request = ProviderRequest {
            prompt: "hang until the boundary cancels".to_string(),
            cwd: Some(temp.path().to_path_buf()),
            ..ProviderRequest::default()
        };

        let started = Instant::now();
        let outcome = complete_provider_phase(
            &router,
            &mut request,
            // This test must prove cleanup of work that actually started. A
            // one-second work window is still short while avoiding a hidden
            // dependency on sub-100 ms process scheduling under suite load.
            ProviderPhaseDeadline::from_now(Duration::from_secs(1), Duration::from_secs(3)),
        )
        .await;
        assert!(matches!(
            outcome,
            ProviderPhaseOutcome::WorkExpired {
                cleanup: ProviderCleanup::Proven
            }
        ));
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(
            request.pid_file.is_none(),
            "phase-owned PID authority leaked into the caller request"
        );
        let descendant = std::fs::read_to_string(&descendant_path)
            .expect("descendant pid")
            .trim()
            .parse::<u32>()
            .expect("numeric descendant pid");
        assert!(
            !deadreckon_core::pid_is_alive(descendant),
            "provider descendant survived the phase deadline"
        );
    }
}

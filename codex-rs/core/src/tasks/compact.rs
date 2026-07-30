use std::sync::Arc;

use super::SessionTask;
use super::SessionTaskResult;
use super::emit_compact_metric;
use crate::session::TurnInput;
use crate::session::session::Session;
use crate::session::turn_context::TurnContext;
use crate::state::TaskKind;
use codex_features::Feature;
use codex_model_provider::RemoteCompactionSupport;
use codex_protocol::error::CodexErrorDetails;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Default)]
pub(crate) struct CompactTask;

impl SessionTask for CompactTask {
    fn kind(&self) -> TaskKind {
        TaskKind::Compact
    }

    fn span_name(&self) -> &'static str {
        "session_task.compact"
    }

    async fn run(
        self: Arc<Self>,
        session: Arc<Session>,
        ctx: Arc<TurnContext>,
        _input: Vec<TurnInput>,
        _cancellation_token: CancellationToken,
    ) -> SessionTaskResult {
        let _profile_guard = ctx.turn_timing_state.begin_compaction();
        if ctx.config.features.enabled(Feature::TokenBudget) {
            crate::compact_token_budget::run_manual_compact_task(session, ctx).await?;
            return Ok(None);
        }

        let mut client_session = session.services.model_client.new_session();
        let mut use_remote = crate::compact::should_use_remote_compact_task_with_offload_policy(
            ctx.provider.info(),
            session.services.model_client.offload_ever_used(),
            client_session.local_offload_enabled_for_turns(),
            client_session.effective_model_offload_compaction_policy(),
        );
        if !use_remote {
            if let Err(err) = crate::session::turn::maybe_recover_remote_compaction_for_local_route(
                &session,
                &ctx,
                &mut client_session,
            )
            .await
            {
                tracing::warn!(
                    error = %err,
                    "manual local compaction recovery failed; falling back to primary compaction"
                );
                use_remote = true;
            } else if crate::compaction_recovery::active_history_has_remote_compaction(
                &session.clone_history().await.into_raw_items(),
            ) {
                use_remote = true;
            }
        }
        let result = if use_remote {
            match ctx.provider.capabilities().remote_compaction {
                RemoteCompactionSupport::V2
                    if ctx.config.features.enabled(Feature::RemoteCompactionV2) =>
                {
                emit_compact_metric(
                    &session.services.session_telemetry,
                    "remote_v2",
                    /*manual*/ true,
                );
                crate::compact_remote_v2::run_remote_compact_task(session.clone(), ctx).await
                }
                RemoteCompactionSupport::V1 | RemoteCompactionSupport::V2 => {
                    emit_compact_metric(
                        &session.services.session_telemetry,
                        "remote",
                        /*manual*/ true,
                    );
                    crate::compact_remote::run_remote_compact_task(session.clone(), ctx).await
                }
                RemoteCompactionSupport::Unsupported => {
                    emit_compact_metric(
                        &session.services.session_telemetry,
                        "local",
                        /*manual*/ true,
                    );
                    crate::compact::run_compact_task(session.clone(), ctx).await
                }
            }
        } else {
            emit_compact_metric(
                &session.services.session_telemetry,
                "local",
                /*manual*/ true,
            );
            crate::compact::run_compact_task(session.clone(), ctx).await
        };
        if let Err(err) = result
            && matches!(err.details(), CodexErrorDetails::TurnAborted)
        {
            return Err(err);
        }
        Ok(None)
    }
}

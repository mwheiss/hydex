use std::sync::Arc;

use super::SessionTask;
use super::SessionTaskContext;
use super::emit_compact_metric;
use crate::session::TurnInput;
use crate::session::turn_context::TurnContext;
use crate::state::TaskKind;
use codex_protocol::user_input::UserInput;
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
        session: Arc<SessionTaskContext>,
        ctx: Arc<TurnContext>,
        _input: Vec<TurnInput>,
        _cancellation_token: CancellationToken,
    ) -> Option<String> {
        let session = session.clone_session();
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
            } else if !client_session.local_compaction_effective() {
                use_remote = true;
            }
        }
        let _ = if use_remote {
            if ctx
                .config
                .features
                .enabled(codex_features::Feature::RemoteCompactionV2)
            {
                emit_compact_metric(
                    &session.services.session_telemetry,
                    "remote_v2",
                    /*manual*/ true,
                );
                crate::compact_remote_v2::run_remote_compact_task(session.clone(), ctx).await
            } else {
                emit_compact_metric(
                    &session.services.session_telemetry,
                    "remote",
                    /*manual*/ true,
                );
                crate::compact_remote::run_remote_compact_task(session.clone(), ctx).await
            }
        } else {
            emit_compact_metric(
                &session.services.session_telemetry,
                "local",
                /*manual*/ true,
            );
            let input = vec![UserInput::Text {
                text: crate::compact::local_compaction_prompt(&ctx).to_string(),
                // Compaction prompt is synthesized; no UI element ranges to preserve.
                text_elements: Vec::new(),
            }];
            crate::compact::run_compact_task(session.clone(), ctx, input).await
        };
        None
    }
}

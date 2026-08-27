use super::*;
use crate::ai::agent::AIAgentContext;
use crate::ai::blocklist::agent_view::AgentViewEntryOrigin;
use crate::test_util::terminal::{
    add_window_with_id_and_terminal, initialize_app_for_terminal_view,
};
use warpui::App;

#[derive(Clone, Copy)]
struct AttachmentArgs<'a> {
    bar: &'a AgentMessageBar,
    terminal: &'a TerminalModel,
    ctx: &'a AppContext,
}

impl AttachedContextArgs for AttachmentArgs<'_> {
    fn terminal_model(&self) -> &TerminalModel {
        self.terminal
    }

    fn input_buffer_model(&self) -> &InputBufferModel {
        self.bar.input_buffer_model.as_ref(self.ctx)
    }

    fn input_model(&self) -> &BlocklistAIInputModel {
        self.bar.input_model.as_ref(self.ctx)
    }

    fn agent_view_controller(&self) -> &AgentViewController {
        self.bar.agent_view_controller.as_ref(self.ctx)
    }

    fn context_model(&self) -> &BlocklistAIContextModel {
        self.bar.context_model.as_ref(self.ctx)
    }

    fn mouse_states(&self) -> &AgentMessageBarMouseStates {
        &self.bar.mouse_states
    }
}

#[test]
fn smash_paperclip_shows_manual_attachment_and_keeps_output_in_context() {
    App::test((), |mut app| async move {
        initialize_app_for_terminal_view(&mut app);
        let _agent_view = FeatureFlag::AgentView.override_enabled(true);
        let _agent_mode = FeatureFlag::AgentMode.override_enabled(true);
        let _auto_context = FeatureFlag::AgentViewBlockContext.override_enabled(true);
        let (window_id, terminal) = add_window_with_id_and_terminal(&mut app, None);

        let block_id = terminal.update(&mut app, |view, ctx| {
            let conversation_id = view.agent_view_controller().update(ctx, |controller, ctx| {
                controller
                    .try_enter_agent_view(
                        None,
                        AgentViewEntryOrigin::Input {
                            was_prompt_autodetected: false,
                        },
                        ctx,
                    )
                    .unwrap()
            });
            let (block_index, block_id) = {
                let mut model = view.model.lock();
                model.simulate_block("printf attachment-check", "SMASH_PAPERCLIP_OUTPUT_827");
                let block = model
                    .block_list()
                    .blocks()
                    .iter()
                    .find(|block| block.command_to_string() == "printf attachment-check")
                    .unwrap();
                let result = (block.index(), block.id().clone());
                model.block_list_mut().associate_blocks_with_conversation(
                    std::iter::once(&result.1),
                    conversation_id,
                );
                result
            };
            view.handle_action(&TerminalAction::AskAIAssistant { block_index }, ctx);
            block_id
        });

        let bars = app.views_of_type::<AgentMessageBar>(window_id).unwrap();
        let bar = bars.last().unwrap();
        for prompt in ["", "Explain this output"] {
            terminal.update(&mut app, |view, ctx| {
                view.input()
                    .update(ctx, |input, ctx| input.replace_buffer_content(prompt, ctx));
            });
            bar.read(&app, |bar, ctx| {
                let context_model = bar.context_model.as_ref(ctx);
                assert!(context_model.pending_context_block_ids().contains(&block_id));
                let context = context_model.pending_context(ctx, true, None);
                assert!(context.iter().any(|item| matches!(item,
                    AIAgentContext::Block(block) if block.output.contains("SMASH_PAPERCLIP_OUTPUT_827")
                        && !block.is_auto_attached
                )), "the paperclip must include terminal output in the next user query");

                let model = bar.terminal_model.lock();
                let message = AttachedBlocksMessageProducer.produce_message(AttachmentArgs {
                    bar, terminal: &model, ctx,
                }).expect("manual paperclip attachments must be visible above the input even with auto-context enabled");
                assert!(message.items.iter().any(|item| matches!(item,
                    MessageItem::Text { content, .. } if content.contains("attached as context")
                        && content.contains("attachment-check")
                )));
            });
        }

        terminal.update(&mut app, |view, ctx| {
            view.input()
                .update(ctx, |input, ctx| input.clear_attached_context(ctx));
        });
        bar.read(&app, |bar, ctx| {
            let model = bar.terminal_model.lock();
            assert!(
                AttachedBlocksMessageProducer
                    .produce_message(AttachmentArgs {
                        bar,
                        terminal: &model,
                        ctx,
                    })
                    .is_none()
            );
            assert!(
                bar.context_model
                    .as_ref(ctx)
                    .pending_context_block_ids()
                    .is_empty()
            );
        });
    });
}

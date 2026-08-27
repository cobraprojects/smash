//! Shared message producers for displaying attached blocks/text context.

use warpui::keymap::Keystroke;

use crate::ai::blocklist::agent_view::{AgentMessageBarMouseStates, AgentViewController};
use crate::ai::blocklist::{BlocklistAIContextModel, BlocklistAIInputModel};
use crate::terminal::input::InputAction;
use crate::terminal::input::buffer_model::InputBufferModel;
use crate::terminal::input::message_bar::{
    Message, MessageItem, MessageProvider, MessageTransformer,
};

/// Trait for message args that can provide attached context information.
/// Exposes the required dependencies for attached context message producers.
pub trait AttachedContextArgs {
    fn input_buffer_model(&self) -> &InputBufferModel;
    fn input_model(&self) -> &BlocklistAIInputModel;
    fn agent_view_controller(&self) -> &AgentViewController;
    fn context_model(&self) -> &BlocklistAIContextModel;
    fn mouse_states(&self) -> &AgentMessageBarMouseStates;
}

/// Produces a message when blocks or selected text are attached as context.
pub struct AttachedBlocksMessageProducer;

/// Labels explicitly attached blocks, excluding automatically included context.
pub fn attached_blocks_label(count: usize) -> String {
    if count == 1 {
        "1 block attached".to_owned()
    } else {
        format!("{count} blocks attached")
    }
}

impl<Args: AttachedContextArgs + Copy> MessageProvider<Args> for AttachedBlocksMessageProducer {
    fn produce_message(&self, args: Args) -> Option<Message> {
        // In the agent view, only show the attached context message if in AI mode.
        if args.agent_view_controller().is_active()
            && !args.input_buffer_model().current_value().is_empty()
            && !args.input_model().is_ai_input_enabled()
        {
            return None;
        }

        // Explicit selections need confirmation even when other blocks are auto-attached.
        let context_block_ids = args.context_model().pending_context_block_ids();
        if context_block_ids.is_empty() {
            return None;
        }

        let mut items = vec![MessageItem::text(attached_blocks_label(
            context_block_ids.len(),
        ))];

        // Always show ESC hint in agent view, make it clickable
        if args.agent_view_controller().is_active() {
            items.push(MessageItem::text(", "));
            items.push(MessageItem::clickable(
                vec![
                    MessageItem::keystroke(Keystroke {
                        key: "escape".to_owned(),
                        ..Default::default()
                    }),
                    MessageItem::text(" to remove"),
                ],
                |ctx| {
                    ctx.dispatch_typed_action(InputAction::ClearAttachedContext);
                },
                args.mouse_states().clear_attached_context.clone(),
            ));
        }

        Some(Message::new(items))
    }
}

/// Keeps attachment status alongside the input's existing helpers and messages.
pub struct AttachedContextMessageTransformer;

impl<Args: AttachedContextArgs + Copy> MessageTransformer<Args>
    for AttachedContextMessageTransformer
{
    fn transform_message(&self, message: &mut Message, args: Args) -> bool {
        let Some(mut attachment) = AttachedBlocksMessageProducer
            .produce_message(args)
            .or_else(|| AttachedTextSelectionMessageProducer.produce_message(args))
        else {
            return false;
        };
        if !message.items.is_empty() {
            attachment.items.push(MessageItem::text(" · "));
            attachment.items.append(&mut message.items);
        }
        *message = attachment;
        true
    }
}

/// Produces a message when text selection is attached as context.
pub struct AttachedTextSelectionMessageProducer;

impl<Args: AttachedContextArgs + Copy> MessageProvider<Args>
    for AttachedTextSelectionMessageProducer
{
    fn produce_message(&self, args: Args) -> Option<Message> {
        // Only apply the visibility condition when agent view is active.
        // When inactive, always show the message.
        if args.agent_view_controller().is_active()
            && !args.input_buffer_model().current_value().is_empty()
            && !args.input_model().is_ai_input_enabled()
        {
            return None;
        }

        // Only show if there's selected text and no blocks attached
        // (blocks take precedence per requirements)
        if !args.context_model().pending_context_block_ids().is_empty() {
            return None;
        }

        let _ = args.context_model().pending_context_selected_text()?;

        let mut items = vec![MessageItem::text("selected text attached as context")];

        // Always show ESC hint in agent view, make it clickable
        if args.agent_view_controller().is_active() {
            items.push(MessageItem::text(", "));
            items.push(MessageItem::clickable(
                vec![
                    MessageItem::keystroke(Keystroke {
                        key: "escape".to_owned(),
                        ..Default::default()
                    }),
                    MessageItem::text(" to remove"),
                ],
                |ctx| {
                    ctx.dispatch_typed_action(InputAction::ClearAttachedContext);
                },
                args.mouse_states().clear_attached_context.clone(),
            ));
        }

        Some(Message::new(items))
    }
}

use super::*;
use crate::ai::agent::AIAgentContext;
use crate::ai::agent::task::TaskId;
use crate::ai::agent::{
    AIAgentActionResult, AIAgentAttachment, RequestCommandOutputResult, UserQueryMode,
};
use prost::Message;

fn query(text: &str) -> AIAgentInput {
    AIAgentInput::UserQuery {
        query: text.to_owned(),
        context: Arc::from([]),
        static_query_type: None,
        referenced_attachments: HashMap::new(),
        user_query_mode: UserQueryMode::Normal,
        running_command: None,
        intended_agent: None,
    }
}

fn saved_task(messages: Vec<api::Message>) -> api::Task {
    let task = api::Task {
        id: "task".to_owned(),
        messages,
        ..Default::default()
    };
    // Persistence stores this proto transcript, not the provider's in-memory messages.
    api::Task::decode(task.encode_to_vec().as_slice()).unwrap()
}

#[test]
fn sends_attached_terminal_context_to_the_model() {
    let input = AIAgentInput::AutoCodeDiffQuery {
        query: "Explain the attached output".to_owned(),
        context: Arc::from([AIAgentContext::SelectedText(
            "SMASH_ATTACHMENT_REGRESSION_731".to_owned(),
        )]),
    };
    let mut messages = Vec::new();
    append_input(&mut messages, &[input]);
    assert!(
        messages[0]
            .to_string()
            .contains("SMASH_ATTACHMENT_REGRESSION_731")
    );
}

#[test]
fn smash_paperclip_output_survives_history_and_reaches_each_provider() {
    let block = super::super::convert_conversation::convert_executed_shell_command(
        api::ExecutedShellCommand {
            command: "printf attachment-check".to_owned(),
            output: "SMASH_PAPERCLIP_OUTPUT_827".to_owned(),
            command_id: "paperclip_block".to_owned(),
            ..Default::default()
        },
    );
    let mut input = query("Explain the attached output");
    if let AIAgentInput::UserQuery { context, .. } = &mut input {
        *context = Arc::from([AIAgentContext::Block(Box::new(block))]);
    }
    let task = saved_task(transcript::input_messages(&[input], "task", "first").unwrap());
    let followup = transcript::input_messages(&[query("Explain further")], "task", "next").unwrap();
    let messages =
        transcript::model_messages(task.messages.iter().chain(&followup), &HashMap::new());
    for payload in [
        serde_json::to_string(&messages).unwrap(),
        serde_json::to_string(&ollama_chat_messages(&messages)).unwrap(),
        serde_json::to_string(&chat_completion_messages(&messages)).unwrap(),
    ] {
        assert!(payload.contains("SMASH_PAPERCLIP_OUTPUT_827"));
        assert!(payload.contains("printf attachment-check"));
        assert!(payload.contains("Explain further"));
    }
}

#[test]
fn saved_transcript_keeps_prompts_attachments_tool_results_and_followups() {
    let mut first = query("What does this output mean?");
    if let AIAgentInput::UserQuery {
        referenced_attachments,
        ..
    } = &mut first
    {
        referenced_attachments.insert(
            "output".to_owned(),
            AIAgentAttachment::Block(
                super::super::convert_conversation::convert_executed_shell_command(
                    api::ExecutedShellCommand {
                        command: "printf marker".to_owned(),
                        output: "SMASH_TERMINAL_OUTPUT_942".to_owned(),
                        command_id: "block_942".to_owned(),
                        ..Default::default()
                    },
                ),
            ),
        );
    }
    let mut messages = transcript::input_messages(&[first], "task", "first").unwrap();
    messages.push(
        tool_call_message(
            "task",
            "first",
            &json!({
                "id": "call_1", "name": "run_shell_command", "input": {"command": "pwd"}
            }),
            &HashMap::new(),
        )
        .unwrap(),
    );
    messages.extend(
        transcript::input_messages(
            &[AIAgentInput::ActionResult {
                result: AIAgentActionResult {
                    id: "call_1".to_owned().into(),
                    task_id: TaskId::new("task".to_owned()),
                    result: AIAgentActionResultType::RequestCommandOutput(
                        RequestCommandOutputResult::Completed {
                            block_id: "block_943".to_owned().into(),
                            command: "pwd".to_owned(),
                            output: "SMASH_TOOL_OUTPUT_943".to_owned(),
                            exit_code: 0.into(),
                            start_ts: None,
                            completed_ts: None,
                        },
                    ),
                },
                context: Arc::from([]),
            }],
            "task",
            "result",
        )
        .unwrap(),
    );
    messages.push(agent_text_message(
        "task",
        "result",
        "Earlier assistant answer",
    ));
    let task = saved_task(messages);
    let restored_inputs = super::super::user_inputs_from_messages(&task.messages);
    let AIAgentInput::UserQuery {
        referenced_attachments,
        ..
    } = &restored_inputs[0]
    else {
        panic!("restored query");
    };
    let AIAgentAttachment::Block(block) = &referenced_attachments["output"] else {
        panic!("restored block");
    };
    assert_eq!(block.output, "SMASH_TERMINAL_OUTPUT_942");
    let next =
        transcript::input_messages(&[query("Explain that further")], "task", "next").unwrap();
    let model = transcript::model_messages(task.messages.iter().chain(&next), &HashMap::new());
    let text = serde_json::to_string(&model).unwrap();
    for expected in [
        "What does this output mean?",
        "SMASH_TERMINAL_OUTPUT_942",
        "SMASH_TOOL_OUTPUT_943",
        "Earlier assistant answer",
        "Explain that further",
        "pwd",
    ] {
        assert!(text.contains(expected), "missing {expected}");
    }
    assert_eq!(
        model
            .iter()
            .filter(|m| m["type"] == "function_call")
            .count(),
        1
    );
    assert_eq!(
        model
            .iter()
            .filter(|m| m["type"] == "function_call_output")
            .count(),
        1
    );
    let ollama = ollama_chat_messages(&model);
    assert!(
        serde_json::to_string(&ollama)
            .unwrap()
            .contains("SMASH_TERMINAL_OUTPUT_942")
    );
    assert!(
        serde_json::to_string(&ollama)
            .unwrap()
            .contains("SMASH_TOOL_OUTPUT_943")
    );
    let compatible = chat_completion_messages(&model);
    assert!(
        serde_json::to_string(&compatible)
            .unwrap()
            .contains("SMASH_TERMINAL_OUTPUT_942")
    );
}

#[test]
fn new_chat_has_no_context_from_another_chat() {
    let first = transcript::input_messages(&[query("PRIVATE_OLD_CONTEXT")], "old", "old").unwrap();
    let _ = transcript::model_messages(first.iter(), &HashMap::new());
    let fresh = transcript::input_messages(&[query("Start fresh")], "new", "new").unwrap();
    let model = transcript::model_messages(fresh.iter(), &HashMap::new());
    assert_eq!(model.len(), 1);
    assert!(
        !serde_json::to_string(&model)
            .unwrap()
            .contains("PRIVATE_OLD_CONTEXT")
    );
}

#[test]
fn interrupted_tool_call_can_be_followed_up_after_restore() {
    let task = saved_task(vec![
        tool_call_message(
            "task",
            "first",
            &json!({
                "id": "interrupted", "name": "run_shell_command", "input": {"command": "pwd"}
            }),
            &HashMap::new(),
        )
        .unwrap(),
    ]);
    let next = transcript::input_messages(&[query("Continue")], "task", "next").unwrap();
    let model = transcript::model_messages(task.messages.iter().chain(&next), &HashMap::new());
    assert_eq!(model[0]["type"], "function_call");
    assert_eq!(model[1]["type"], "function_call_output");
    assert_eq!(model[1]["call_id"], "interrupted");
    assert!(model[1]["output"].as_str().unwrap().contains("Interrupted"));
    assert_eq!(model[2]["role"], "user");
}

#[test]
fn records_prompt_before_contacting_the_provider() {
    use futures_lite::StreamExt;
    futures_lite::future::block_on(async {
        let mut params = RequestParams::new_for_test();
        params.input = vec![query("Persist even if the request is stopped")];
        let (_cancel_tx, cancel_rx) = futures::channel::oneshot::channel();
        let mut stream = generate_output(params, cancel_rx).await.unwrap();
        assert!(matches!(
            stream.next().await.unwrap().unwrap().r#type,
            Some(api::response_event::Type::Init(_))
        ));
        let Some(api::response_event::Type::ClientActions(created)) =
            stream.next().await.unwrap().unwrap().r#type
        else {
            panic!("created task");
        };
        assert!(matches!(
            created.actions[0].action,
            Some(api::client_action::Action::CreateTask(_))
        ));
        let Some(api::response_event::Type::ClientActions(recorded)) =
            stream.next().await.unwrap().unwrap().r#type
        else {
            panic!("persisted prompt");
        };
        let Some(api::client_action::Action::AddMessagesToTask(added)) =
            &recorded.actions[0].action
        else {
            panic!("input message");
        };
        let Some(api::message::Message::UserQuery(input)) = &added.messages[0].message else {
            panic!("user query");
        };
        assert_eq!(input.query, "Persist even if the request is stopped");
        // Dropping here never polls the provider request. This test needs no credentials/network.
    });
}

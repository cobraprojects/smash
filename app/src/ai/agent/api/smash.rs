use std::collections::HashMap;
use std::sync::{Arc, LazyLock};

use anyhow::anyhow;
use parking_lot::Mutex;
use serde_json::{Value, json};
use uuid::Uuid;
use warp_multi_agent_api as api;

use super::{ConvertToAPITypeError, RequestParams, ResponseStream};
use crate::ai::agent::{AIAgentActionResultType, AIAgentInput};
use crate::ai::llms::{smash_lm_studio_url, smash_ollama_url};
use crate::ai::smash_chatgpt;
use crate::server::server_api::AIApiError;

const DEFAULT_MODEL: &str = "gpt-5.6-sol";

static CONVERSATIONS: LazyLock<Mutex<HashMap<String, Vec<Value>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

pub(super) async fn generate_output(
    params: RequestParams,
    cancellation_rx: futures::channel::oneshot::Receiver<()>,
) -> Result<ResponseStream, ConvertToAPITypeError> {
    let conversation_id = params
        .conversation_token
        .as_ref()
        .map(|token| token.as_str().to_owned())
        .unwrap_or_else(|| Uuid::new_v4().to_string());
    let request_id = Uuid::new_v4().to_string();
    let task_id = params
        .tasks
        .first()
        .map(|task| task.id.clone())
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    let input = input_text(&params.input);
    let mut messages = CONVERSATIONS
        .lock()
        .get(&conversation_id)
        .cloned()
        .unwrap_or_default();
    append_input(&mut messages, &params.input, input);

    let model = normalize_model(params.model.as_str());
    let request = json!({
        "model": model,
        "instructions": system_prompt(),
        "input": messages,
        "tools": tools(),
        "parallel_tool_calls": true,
        "store": false,
        "stream": true,
    });

    let result = tokio::select! {
        _ = cancellation_rx => Err(anyhow!("request cancelled")),
        result = send_provider_request(&model, &request, &messages) => result,
    };

    let mut events = vec![stream_init(&conversation_id, &request_id)];
    match result {
        Ok(content) => {
            if params.tasks.is_empty() {
                events.push(client_action(api::client_action::Action::CreateTask(
                    api::client_action::CreateTask {
                        task: Some(api::Task {
                            id: task_id.clone(),
                            description: "Smash agent".to_owned(),
                            ..Default::default()
                        }),
                    },
                )));
            }

            append_output(&mut messages, &content);
            CONVERSATIONS
                .lock()
                .insert(conversation_id.clone(), messages);

            let mut output_messages = Vec::new();
            output_messages.push(model_used_message(&task_id, &request_id, &model));
            for block in content {
                match block.get("type").and_then(Value::as_str) {
                    Some("text") => {
                        if let Some(text) = block.get("text").and_then(Value::as_str)
                            && !text.is_empty()
                        {
                            output_messages.push(agent_text_message(&task_id, &request_id, text));
                        }
                    }
                    Some("tool_use") => {
                        if let Some(message) = tool_call_message(&task_id, &request_id, &block) {
                            output_messages.push(message);
                        }
                    }
                    _ => {}
                }
            }
            if !output_messages.is_empty() {
                events.push(add_messages(&task_id, output_messages));
            }
            events.push(stream_finished());
        }
        Err(error) => {
            let error = Arc::new(AIApiError::Other(error));
            let (tx, rx) = async_channel::unbounded();
            for event in events {
                let _ = tx.send(Ok(event)).await;
            }
            let _ = tx.send(Err(error)).await;
            return Ok(Box::pin(rx));
        }
    }

    Ok(Box::pin(futures_lite::stream::iter(
        events.into_iter().map(Ok),
    )))
}

fn normalize_model(model: &str) -> String {
    if model.starts_with("ollama:") {
        return model.to_owned();
    }
    if model.starts_with("lmstudio:") {
        return model.to_owned();
    }
    match model {
        "gpt-5.4" | "claude-gpt-5.4" => "gpt-5.4",
        "gpt-5.5" | "claude-gpt-5.5" => "gpt-5.5",
        "gpt-5.6-luna" | "claude-gpt-5.6-luna" => "gpt-5.6-luna",
        "gpt-5.6-terra" | "claude-gpt-5.6-terra" => "gpt-5.6-terra",
        "gpt-5.6-sol" | "claude-gpt-5.6-sol" => "gpt-5.6-sol",
        _ => DEFAULT_MODEL,
    }
    .to_owned()
}

async fn send_provider_request(
    model: &str,
    chatgpt_request: &Value,
    messages: &[Value],
) -> anyhow::Result<Vec<Value>> {
    if let Some(model) = model.strip_prefix("ollama:") {
        send_ollama_request(model, messages).await
    } else if let Some(model) = model.strip_prefix("lmstudio:") {
        send_lm_studio_request(model, messages).await
    } else {
        smash_chatgpt::send_responses(chatgpt_request).await
    }
}

async fn send_lm_studio_request(model: &str, messages: &[Value]) -> anyhow::Result<Vec<Value>> {
    let chat_messages = chat_completion_messages(messages);
    let tools = tools()
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|tool| {
            json!({
                "type": "function",
                "function": {
                    "name": tool.get("name").cloned().unwrap_or(Value::Null),
                    "description": tool.get("description").cloned().unwrap_or(Value::Null),
                    "parameters": tool.get("parameters").cloned().unwrap_or_else(|| json!({})),
                }
            })
        })
        .collect::<Vec<_>>();
    let response = reqwest::Client::new()
        .post(format!("{}/v1/chat/completions", smash_lm_studio_url()))
        .json(&json!({
            "model": model,
            "messages": chat_messages,
            "tools": tools,
            "stream": false,
        }))
        .send()
        .await
        .map_err(|error| anyhow!("Could not connect to LM Studio: {error}"))?;
    if !response.status().is_success() {
        return Err(anyhow!("LM Studio returned {}", response.status()));
    }
    let response: Value = response.json().await?;
    let message = response
        .pointer("/choices/0/message")
        .ok_or_else(|| anyhow!("LM Studio returned no message"))?;
    let mut output = Vec::new();
    if let Some(text) = message.get("content").and_then(Value::as_str)
        && !text.is_empty()
    {
        output.push(json!({ "type": "text", "text": text }));
    }
    for tool_call in message
        .get("tool_calls")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let function = tool_call.get("function").unwrap_or(tool_call);
        let arguments = function
            .get("arguments")
            .and_then(Value::as_str)
            .and_then(|arguments| serde_json::from_str(arguments).ok())
            .unwrap_or_else(|| json!({}));
        output.push(json!({
            "type": "tool_use",
            "id": tool_call.get("id").and_then(Value::as_str).unwrap_or_default(),
            "name": function.get("name").and_then(Value::as_str).unwrap_or_default(),
            "input": arguments,
        }));
    }
    Ok(output)
}

async fn send_ollama_request(model: &str, messages: &[Value]) -> anyhow::Result<Vec<Value>> {
    let ollama_messages = ollama_chat_messages(messages);
    let ollama_tools = tools()
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|tool| {
            json!({
                "type": "function",
                "function": {
                    "name": tool.get("name").cloned().unwrap_or(Value::Null),
                    "description": tool.get("description").cloned().unwrap_or(Value::Null),
                    "parameters": tool.get("parameters").cloned().unwrap_or_else(|| json!({})),
                }
            })
        })
        .collect::<Vec<_>>();
    let response = reqwest::Client::new()
        .post(format!("{}/api/chat", smash_ollama_url()))
        .json(&json!({
            "model": model,
            "messages": ollama_messages,
            "tools": ollama_tools,
            "stream": false,
        }))
        .send()
        .await
        .map_err(|error| anyhow!("Could not connect to Ollama: {error}"))?;
    if !response.status().is_success() {
        let status = response.status();
        let detail = response.text().await.unwrap_or_default();
        return Err(anyhow!("Ollama returned {status}: {detail}"));
    }
    let response: Value = response.json().await?;
    let message = response
        .get("message")
        .ok_or_else(|| anyhow!("Ollama returned no message"))?;
    let mut output = Vec::new();
    if let Some(text) = message.get("content").and_then(Value::as_str)
        && !text.is_empty()
    {
        output.push(json!({ "type": "text", "text": text }));
    }
    for tool_call in message
        .get("tool_calls")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let function = tool_call.get("function").unwrap_or(tool_call);
        output.push(json!({
            "type": "tool_use",
            "id": Uuid::new_v4().to_string(),
            "name": function.get("name").and_then(Value::as_str).unwrap_or_default(),
            "input": function.get("arguments").cloned().unwrap_or_else(|| json!({})),
        }));
    }
    Ok(output)
}

fn ollama_chat_messages(messages: &[Value]) -> Vec<Value> {
    let mut tool_names = HashMap::new();
    let mut result = Vec::new();
    for message in messages {
        if message.get("type").and_then(Value::as_str) == Some("function_call") {
            let call_id = message
                .get("call_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let name = message
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            tool_names.insert(call_id.to_owned(), name.to_owned());
            result.push(json!({
                "role": "assistant",
                "content": "",
                "tool_calls": [{
                    "function": {
                        "name": name,
                        "arguments": message
                            .get("arguments")
                            .and_then(Value::as_str)
                            .and_then(|arguments| serde_json::from_str::<Value>(arguments).ok())
                            .unwrap_or_else(|| json!({})),
                    }
                }]
            }));
            continue;
        }
        if message.get("type").and_then(Value::as_str) == Some("function_call_output") {
            let call_id = message
                .get("call_id")
                .and_then(Value::as_str)
                .unwrap_or_default();
            result.push(json!({
                "role": "tool",
                "tool_name": tool_names.get(call_id).cloned().unwrap_or_default(),
                "content": message.get("output").cloned().unwrap_or(Value::Null),
            }));
            continue;
        }
        if let Some(role) = message.get("role").and_then(Value::as_str) {
            let content = message
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("");
            result.push(json!({ "role": role, "content": content }));
        }
    }
    result
}

fn chat_completion_messages(messages: &[Value]) -> Vec<Value> {
    messages
        .iter()
        .filter_map(|message| {
            if message.get("type").and_then(Value::as_str) == Some("function_call") {
                return Some(json!({
                    "role": "assistant",
                    "content": "",
                    "tool_calls": [{
                        "id": message.get("call_id").cloned().unwrap_or(Value::Null),
                        "type": "function",
                        "function": {
                            "name": message.get("name").cloned().unwrap_or(Value::Null),
                            "arguments": message.get("arguments").cloned().unwrap_or(Value::Null),
                        }
                    }]
                }));
            }
            if message.get("type").and_then(Value::as_str) == Some("function_call_output") {
                return Some(json!({
                    "role": "tool",
                    "tool_call_id": message.get("call_id").cloned().unwrap_or(Value::Null),
                    "content": message.get("output").cloned().unwrap_or(Value::Null),
                }));
            }
            let role = message.get("role")?.as_str()?;
            let content = message
                .get("content")?
                .as_array()?
                .iter()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("");
            Some(json!({ "role": role, "content": content }))
        })
        .collect()
}

fn append_input(messages: &mut Vec<Value>, inputs: &[AIAgentInput], text: String) {
    if inputs.len() == 1
        && let AIAgentInput::ActionResult { result, .. } = &inputs[0]
    {
        messages.push(json!({
            "type": "function_call_output",
            "call_id": result.id.to_string(),
            "output": action_result_text(&result.result),
        }));
        return;
    }
    messages.push(json!({
        "role": "user",
        "content": [{ "type": "input_text", "text": text }],
    }));
}

fn append_output(messages: &mut Vec<Value>, output: &[Value]) {
    for block in output {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(Value::as_str) {
                    messages.push(json!({
                        "role": "assistant",
                        "content": [{ "type": "output_text", "text": text }],
                    }));
                }
            }
            Some("tool_use") => {
                messages.push(json!({
                    "type": "function_call",
                    "call_id": block.get("id").and_then(Value::as_str).unwrap_or_default(),
                    "name": block.get("name").and_then(Value::as_str).unwrap_or_default(),
                    "arguments": serde_json::to_string(
                        block.get("input").unwrap_or(&Value::Null)
                    ).unwrap_or_else(|_| "{}".to_owned()),
                }));
            }
            _ => {}
        }
    }
}

fn input_text(inputs: &[AIAgentInput]) -> String {
    inputs
        .iter()
        .map(|input| match input {
            AIAgentInput::UserQuery { query, .. }
            | AIAgentInput::AutoCodeDiffQuery { query, .. }
            | AIAgentInput::CreateNewProject { query, .. } => query.clone(),
            AIAgentInput::ActionResult { result, .. } => action_result_text(&result.result),
            AIAgentInput::SummarizeConversation { prompt, .. } => prompt
                .clone()
                .unwrap_or_else(|| "Summarize the conversation.".to_owned()),
            other => format!("{other:?}"),
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn action_result_text(result: &AIAgentActionResultType) -> String {
    format!("Tool result from Smash:\n{result:#?}")
}

fn system_prompt() -> &'static str {
    "You are the native Smash terminal agent. Work directly in the user's current terminal session. Use tools when you need to inspect files, execute commands, or make changes. Explain the result concisely. Never claim a command or file change happened unless a tool result confirms it."
}

fn tools() -> Value {
    json!([
        {
            "type": "function",
            "name": "run_shell_command",
            "description": "Run a shell command in the active Smash terminal session.",
            "parameters": {
                "type": "object",
                "properties": {
                    "command": { "type": "string" }
                },
                "required": ["command"]
            }
        },
        {
            "type": "function",
            "name": "read_files",
            "description": "Read one or more files from the active machine.",
            "parameters": {
                "type": "object",
                "properties": {
                    "paths": { "type": "array", "items": { "type": "string" } }
                },
                "required": ["paths"]
            }
        },
        {
            "type": "function",
            "name": "apply_file_diffs",
            "description": "Replace an exact string in a file. Use one edit per file change.",
            "parameters": {
                "type": "object",
                "properties": {
                    "summary": { "type": "string" },
                    "file_path": { "type": "string" },
                    "search": { "type": "string" },
                    "replace": { "type": "string" }
                },
                "required": ["summary", "file_path", "search", "replace"]
            }
        }
    ])
}

fn tool_call_message(task_id: &str, request_id: &str, block: &Value) -> Option<api::Message> {
    let id = block.get("id")?.as_str()?.to_owned();
    let name = block.get("name")?.as_str()?;
    let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
    let tool = match name {
        "run_shell_command" => {
            let command = input.get("command")?.as_str()?.to_owned();
            api::message::tool_call::Tool::RunShellCommand(
                api::message::tool_call::RunShellCommand {
                    command,
                    is_read_only: false,
                    uses_pager: false,
                    citations: vec![],
                    is_risky: false,
                    wait_until_complete_value: None,
                    risk_category: api::RiskCategory::Unspecified.into(),
                },
            )
        }
        "read_files" => {
            let files = input
                .get("paths")?
                .as_array()?
                .iter()
                .filter_map(Value::as_str)
                .map(|name| api::message::tool_call::read_files::File {
                    name: name.to_owned(),
                    line_ranges: vec![],
                })
                .collect();
            api::message::tool_call::Tool::ReadFiles(api::message::tool_call::ReadFiles { files })
        }
        "apply_file_diffs" => {
            let file_path = input.get("file_path")?.as_str()?.to_owned();
            let search = input.get("search")?.as_str()?.to_owned();
            let replace = input.get("replace")?.as_str()?.to_owned();
            let summary = input
                .get("summary")
                .and_then(Value::as_str)
                .unwrap_or("Update file")
                .to_owned();
            api::message::tool_call::Tool::ApplyFileDiffs(api::message::tool_call::ApplyFileDiffs {
                summary,
                diffs: vec![api::message::tool_call::apply_file_diffs::FileDiff {
                    file_path,
                    search,
                    replace,
                }],
                new_files: vec![],
                deleted_files: vec![],
                v4a_updates: vec![],
            })
        }
        _ => return None,
    };

    Some(api::Message {
        id: Uuid::new_v4().to_string(),
        task_id: task_id.to_owned(),
        request_id: request_id.to_owned(),
        message: Some(api::message::Message::ToolCall(api::message::ToolCall {
            tool_call_id: id,
            tool: Some(tool),
        })),
        ..Default::default()
    })
}

fn stream_init(conversation_id: &str, request_id: &str) -> api::ResponseEvent {
    api::ResponseEvent {
        r#type: Some(api::response_event::Type::Init(
            api::response_event::StreamInit {
                conversation_id: conversation_id.to_owned(),
                request_id: request_id.to_owned(),
                run_id: String::new(),
            },
        )),
    }
}

fn client_action(action: api::client_action::Action) -> api::ResponseEvent {
    api::ResponseEvent {
        r#type: Some(api::response_event::Type::ClientActions(
            api::response_event::ClientActions {
                actions: vec![api::ClientAction {
                    action: Some(action),
                }],
            },
        )),
    }
}

fn add_messages(task_id: &str, messages: Vec<api::Message>) -> api::ResponseEvent {
    client_action(api::client_action::Action::AddMessagesToTask(
        api::client_action::AddMessagesToTask {
            task_id: task_id.to_owned(),
            messages,
        },
    ))
}

fn agent_text_message(task_id: &str, request_id: &str, text: &str) -> api::Message {
    api::Message {
        id: Uuid::new_v4().to_string(),
        task_id: task_id.to_owned(),
        request_id: request_id.to_owned(),
        message: Some(api::message::Message::AgentOutput(
            api::message::AgentOutput {
                text: text.to_owned(),
            },
        )),
        ..Default::default()
    }
}

fn model_used_message(task_id: &str, request_id: &str, model: &str) -> api::Message {
    let display_name = model
        .strip_prefix("claude-")
        .unwrap_or(model)
        .to_uppercase();
    api::Message {
        id: Uuid::new_v4().to_string(),
        task_id: task_id.to_owned(),
        request_id: request_id.to_owned(),
        message: Some(api::message::Message::ModelUsed(api::message::ModelUsed {
            model_id: model.to_owned(),
            model_display_name: display_name,
            is_fallback: false,
            prompt_cache_expires_at: None,
        })),
        ..Default::default()
    }
}

fn stream_finished() -> api::ResponseEvent {
    api::ResponseEvent {
        r#type: Some(api::response_event::Type::Finished(
            api::response_event::StreamFinished {
                reason: Some(api::response_event::stream_finished::Reason::Done(
                    api::response_event::stream_finished::Done {},
                )),
                token_usage: vec![],
                should_refresh_model_config: false,
                request_cost: None,
                conversation_usage_metadata: None,
            },
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_supported_chatgpt_models_are_forwarded() {
        assert_eq!(normalize_model("gpt-5.6-sol"), "gpt-5.6-sol");
        assert_eq!(normalize_model("oz-agent"), DEFAULT_MODEL);
    }
}

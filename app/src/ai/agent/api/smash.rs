use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock};

use anyhow::anyhow;
use parking_lot::Mutex;
use serde_json::{Value, json};
use uuid::Uuid;
use warp_multi_agent_api as api;

use super::{ConvertToAPITypeError, RequestParams, ResponseStream};
use crate::ai::agent::{AIAgentActionResultType, AIAgentInput, MCPContext};
use crate::ai::llms::{smash_lm_studio_url, smash_ollama_url};
use crate::ai::smash_chatgpt;
use crate::server::server_api::AIApiError;

const DEFAULT_MODEL: &str = "gpt-5.6-sol";

static CONVERSATIONS: LazyLock<Mutex<HashMap<String, Vec<Value>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

#[derive(Debug, Clone, PartialEq, Eq)]
struct McpToolRoute {
    server_id: String,
    tool_name: String,
}

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

    let mut messages = CONVERSATIONS
        .lock()
        .get(&conversation_id)
        .cloned()
        .unwrap_or_default();
    append_input(&mut messages, &params.input);

    let model = normalize_model(params.model.as_str());
    let (available_tools, mcp_tool_routes) = tools(params.mcp_context.as_ref());
    let request = json!({
        "model": model,
        "instructions": system_prompt(),
        "input": messages,
        "tools": available_tools.clone(),
        // Smash's action runner returns tool results as they finish. Keep calls sequential so a
        // fast result cannot be sent while another call from the same model response is pending.
        "parallel_tool_calls": false,
        "store": false,
        "stream": true,
    });

    let result = tokio::select! {
        _ = cancellation_rx => Err(anyhow!("request cancelled")),
        result = send_provider_request(&model, &request, &messages, &available_tools) => result,
    };

    let mut events = vec![stream_init(&conversation_id, &request_id)];
    match result {
        Ok(mut content) => {
            keep_only_first_tool_call(&mut content);
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
                        if let Some(message) =
                            tool_call_message(&task_id, &request_id, &block, &mcp_tool_routes)
                        {
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
    available_tools: &[Value],
) -> anyhow::Result<Vec<Value>> {
    if let Some(model) = model.strip_prefix("ollama:") {
        send_ollama_request(model, messages, available_tools).await
    } else if let Some(model) = model.strip_prefix("lmstudio:") {
        send_lm_studio_request(model, messages, available_tools).await
    } else {
        smash_chatgpt::send_responses(chatgpt_request).await
    }
}

async fn send_lm_studio_request(
    model: &str,
    messages: &[Value],
    available_tools: &[Value],
) -> anyhow::Result<Vec<Value>> {
    let chat_messages = chat_completion_messages(messages);
    let tools = available_tools
        .iter()
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
            "parallel_tool_calls": false,
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

async fn send_ollama_request(
    model: &str,
    messages: &[Value],
    available_tools: &[Value],
) -> anyhow::Result<Vec<Value>> {
    let ollama_messages = ollama_chat_messages(messages);
    let ollama_tools = available_tools
        .iter()
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

fn append_input(messages: &mut Vec<Value>, inputs: &[AIAgentInput]) {
    for input in inputs {
        if let AIAgentInput::ActionResult { result, .. } = input {
            messages.push(json!({
                "type": "function_call_output",
                "call_id": result.id.to_string(),
                "output": action_result_text(&result.result),
            }));
        }
    }

    let text = input_text(inputs);
    if text.is_empty() {
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

fn keep_only_first_tool_call(output: &mut Vec<Value>) {
    let mut found_tool_call = false;
    output.retain(|block| {
        if block.get("type").and_then(Value::as_str) != Some("tool_use") {
            return true;
        }
        if found_tool_call {
            return false;
        }
        found_tool_call = true;
        true
    });
}

fn input_text(inputs: &[AIAgentInput]) -> String {
    inputs
        .iter()
        .filter_map(|input| match input {
            AIAgentInput::UserQuery { query, .. }
            | AIAgentInput::AutoCodeDiffQuery { query, .. }
            | AIAgentInput::CreateNewProject { query, .. } => Some(query.clone()),
            AIAgentInput::InvokeSkill {
                skill, user_query, ..
            } => Some(format!(
                "Use the following Smash skill exactly as its instructions require.\n\n\
                 Skill name: {}\nSkill source: {}\n\n{}\n\nUser request:\n{}",
                skill.name,
                skill.path.display_path(),
                skill.content,
                user_query
                    .as_ref()
                    .map(|query| query.query.as_str())
                    .unwrap_or("Run this skill for the current task."),
            )),
            AIAgentInput::ActionResult { .. } => None,
            AIAgentInput::SummarizeConversation { prompt, .. } => Some(
                prompt
                    .clone()
                    .unwrap_or_else(|| "Summarize the conversation.".to_owned()),
            ),
            other => Some(format!("{other:?}")),
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

fn tools(mcp_context: Option<&MCPContext>) -> (Vec<Value>, HashMap<String, McpToolRoute>) {
    let mut tools = vec![
        json!({
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
        }),
        json!({
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
        }),
        json!({
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
        }),
    ];
    let mut routes = HashMap::new();
    let mut used_names = HashSet::from([
        "run_shell_command".to_owned(),
        "read_files".to_owned(),
        "apply_file_diffs".to_owned(),
    ]);

    let Some(mcp_context) = mcp_context else {
        return (tools, routes);
    };

    for server in &mcp_context.servers {
        append_mcp_tools(
            &mut tools,
            &mut routes,
            &mut used_names,
            &server.id,
            &server.name,
            &server.tools,
        );
    }

    if mcp_context.servers.is_empty() {
        #[allow(deprecated)]
        append_mcp_tools(
            &mut tools,
            &mut routes,
            &mut used_names,
            "",
            "MCP",
            &mcp_context.tools,
        );
    }

    (tools, routes)
}

fn append_mcp_tools(
    tools: &mut Vec<Value>,
    routes: &mut HashMap<String, McpToolRoute>,
    used_names: &mut HashSet<String>,
    server_id: &str,
    server_name: &str,
    mcp_tools: &[rmcp::model::Tool],
) {
    for mcp_tool in mcp_tools {
        let tool_name = mcp_tool.name.as_ref();
        let advertised_name = unique_tool_name(tool_name, server_name, used_names);
        let description = mcp_tool
            .description
            .as_deref()
            .map(|description| format!("MCP tool from {server_name}: {description}"))
            .unwrap_or_else(|| format!("MCP tool {tool_name} from {server_name}."));
        tools.push(json!({
            "type": "function",
            "name": advertised_name,
            "description": description,
            "parameters": Value::Object(mcp_tool.input_schema.as_ref().clone()),
        }));
        routes.insert(
            advertised_name,
            McpToolRoute {
                server_id: server_id.to_owned(),
                tool_name: tool_name.to_owned(),
            },
        );
    }
}

fn unique_tool_name(
    tool_name: &str,
    server_name: &str,
    used_names: &mut HashSet<String>,
) -> String {
    let direct_name = sanitize_tool_name(tool_name);
    if direct_name == tool_name && used_names.insert(direct_name.clone()) {
        return direct_name;
    }

    let base = format!("mcp__{}__{}", sanitize_tool_name(server_name), direct_name);
    let mut candidate = truncate_tool_name(&base, 64);
    let mut suffix_number = 2;
    while !used_names.insert(candidate.clone()) {
        let suffix = format!("_{suffix_number}");
        candidate = format!(
            "{}{}",
            truncate_tool_name(&base, 64usize.saturating_sub(suffix.len())),
            suffix
        );
        suffix_number += 1;
    }
    candidate
}

fn sanitize_tool_name(name: &str) -> String {
    let sanitized = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    let sanitized = if sanitized.is_empty() {
        "mcp_tool".to_owned()
    } else {
        sanitized
    };
    truncate_tool_name(&sanitized, 64)
}

fn truncate_tool_name(name: &str, max_chars: usize) -> String {
    name.chars().take(max_chars).collect()
}

fn tool_call_message(
    task_id: &str,
    request_id: &str,
    block: &Value,
    mcp_tool_routes: &HashMap<String, McpToolRoute>,
) -> Option<api::Message> {
    let id = block.get("id")?.as_str()?.to_owned();
    let name = block.get("name")?.as_str()?;
    let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
    let tool = if let Some(route) = mcp_tool_routes.get(name) {
        let args = json_object_to_prost_struct(&input)?;
        api::message::tool_call::Tool::CallMcpTool(api::message::tool_call::CallMcpTool {
            name: route.tool_name.clone(),
            args: Some(args),
            server_id: route.server_id.clone(),
        })
    } else {
        match name {
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
                api::message::tool_call::Tool::ReadFiles(api::message::tool_call::ReadFiles {
                    files,
                })
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
                api::message::tool_call::Tool::ApplyFileDiffs(
                    api::message::tool_call::ApplyFileDiffs {
                        summary,
                        diffs: vec![api::message::tool_call::apply_file_diffs::FileDiff {
                            file_path,
                            search,
                            replace,
                        }],
                        new_files: vec![],
                        deleted_files: vec![],
                        v4a_updates: vec![],
                    },
                )
            }
            _ => return None,
        }
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

fn json_object_to_prost_struct(value: &Value) -> Option<prost_types::Struct> {
    let Value::Object(fields) = value else {
        return None;
    };
    Some(prost_types::Struct {
        fields: fields
            .iter()
            .map(|(name, value)| (name.clone(), json_to_prost_value(value)))
            .collect(),
    })
}

fn json_to_prost_value(value: &Value) -> prost_types::Value {
    use prost_types::value::Kind;

    let kind = match value {
        Value::Null => Kind::NullValue(prost_types::NullValue::NullValue.into()),
        Value::Bool(value) => Kind::BoolValue(*value),
        Value::Number(value) => Kind::NumberValue(value.as_f64().unwrap_or_default()),
        Value::String(value) => Kind::StringValue(value.clone()),
        Value::Array(values) => Kind::ListValue(prost_types::ListValue {
            values: values.iter().map(json_to_prost_value).collect(),
        }),
        Value::Object(fields) => Kind::StructValue(prost_types::Struct {
            fields: fields
                .iter()
                .map(|(name, value)| (name.clone(), json_to_prost_value(value)))
                .collect(),
        }),
    };
    prost_types::Value { kind: Some(kind) }
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
    use ai::skills::{ParsedSkill, SkillProvider, SkillScope};
    use warp_util::local_or_remote_path::LocalOrRemotePath;

    use crate::ai::agent::task::TaskId;
    use crate::ai::agent::{AIAgentActionResult, InvokeSkillUserQuery, MCPServer};

    fn action_result(id: &str) -> AIAgentInput {
        AIAgentInput::ActionResult {
            result: AIAgentActionResult {
                id: id.to_owned().into(),
                task_id: TaskId::new("task-1".to_owned()),
                result: AIAgentActionResultType::InitProject,
            },
            context: Arc::from([]),
        }
    }

    #[test]
    fn only_supported_chatgpt_models_are_forwarded() {
        assert_eq!(normalize_model("gpt-5.6-sol"), "gpt-5.6-sol");
        assert_eq!(normalize_model("oz-agent"), DEFAULT_MODEL);
    }

    #[test]
    fn appends_every_parallel_tool_result_for_chatgpt() {
        let mut messages = Vec::new();
        append_input(
            &mut messages,
            &[action_result("call-1"), action_result("call-2")],
        );

        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0]["type"], "function_call_output");
        assert_eq!(messages[0]["call_id"], "call-1");
        assert_eq!(messages[1]["type"], "function_call_output");
        assert_eq!(messages[1]["call_id"], "call-2");
    }

    #[test]
    fn sends_tool_calls_to_the_action_runner_sequentially() {
        let mut output = vec![
            json!({ "type": "text", "text": "Checking." }),
            json!({ "type": "tool_use", "id": "call-1", "name": "read_files" }),
            json!({ "type": "tool_use", "id": "call-2", "name": "read_files" }),
        ];

        keep_only_first_tool_call(&mut output);

        assert_eq!(output.len(), 2);
        assert_eq!(output[1]["id"], "call-1");
    }

    #[test]
    fn sends_skill_instructions_and_user_request_to_the_model() {
        let input = AIAgentInput::InvokeSkill {
            context: Arc::from([]),
            skill: ParsedSkill {
                path: LocalOrRemotePath::Local("/tmp/.smash/skills/fix-issue/SKILL.md".into()),
                name: "fix-issue".to_owned(),
                description: "Fix an issue".to_owned(),
                content: "Always reproduce the issue first.".to_owned(),
                line_range: None,
                provider: SkillProvider::Smash,
                scope: SkillScope::Project,
            },
            user_query: Some(InvokeSkillUserQuery {
                query: "What does this skill do?".to_owned(),
                referenced_attachments: HashMap::new(),
            }),
        };

        let text = input_text(&[input]);

        assert!(text.contains("Skill name: fix-issue"));
        assert!(text.contains("Always reproduce the issue first."));
        assert!(text.contains("What does this skill do?"));
    }

    fn mcp_tool(name: &str) -> rmcp::model::Tool {
        serde_json::from_value(json!({
            "name": name,
            "description": "Return a smoke-test value.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "value": { "type": "string" }
                },
                "required": ["value"]
            }
        }))
        .expect("valid MCP tool")
    }

    #[test]
    #[allow(deprecated)]
    fn includes_grouped_mcp_tools_in_every_provider_request() {
        let context = MCPContext {
            resources: vec![],
            tools: vec![],
            servers: vec![MCPServer {
                id: "31e95220-cf4e-4d48-9bd0-c8f93bf0057a".to_owned(),
                name: "Smoke server".to_owned(),
                description: String::new(),
                resources: vec![],
                tools: vec![mcp_tool("smash_mcp_probe")],
            }],
        };

        let (available_tools, routes) = tools(Some(&context));

        assert!(
            available_tools
                .iter()
                .any(|tool| tool["name"] == "smash_mcp_probe")
        );
        assert_eq!(
            routes.get("smash_mcp_probe"),
            Some(&McpToolRoute {
                server_id: "31e95220-cf4e-4d48-9bd0-c8f93bf0057a".to_owned(),
                tool_name: "smash_mcp_probe".to_owned(),
            })
        );
    }

    #[test]
    fn converts_model_mcp_call_to_client_mcp_action() {
        let routes = HashMap::from([(
            "smash_mcp_probe".to_owned(),
            McpToolRoute {
                server_id: "31e95220-cf4e-4d48-9bd0-c8f93bf0057a".to_owned(),
                tool_name: "smash_mcp_probe".to_owned(),
            },
        )]);
        let block = json!({
            "type": "tool_use",
            "id": "call-1",
            "name": "smash_mcp_probe",
            "input": { "value": "live-test", "nested": [true, 3] }
        });

        let message = tool_call_message("task-1", "request-1", &block, &routes)
            .expect("MCP call should be converted");
        let Some(api::message::Message::ToolCall(tool_call)) = message.message else {
            panic!("expected tool call message");
        };
        let Some(api::message::tool_call::Tool::CallMcpTool(call)) = tool_call.tool else {
            panic!("expected MCP tool call");
        };
        assert_eq!(call.name, "smash_mcp_probe");
        assert_eq!(call.server_id, "31e95220-cf4e-4d48-9bd0-c8f93bf0057a");
        let args = call.args.expect("MCP args");
        assert_eq!(
            args.fields["value"].kind,
            Some(prost_types::value::Kind::StringValue(
                "live-test".to_owned()
            ))
        );
    }

    #[test]
    fn namespaces_duplicate_mcp_tool_names() {
        let mut used = HashSet::from(["duplicate".to_owned()]);

        let name = unique_tool_name("duplicate", "Second server", &mut used);

        assert_eq!(name, "mcp__Second_server__duplicate");
    }
}
